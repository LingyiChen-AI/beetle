use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use reqwest::{Client as HttpClient, RequestBuilder, Url};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{Mutex, watch},
    time::{Instant, sleep, sleep_until, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const API: &str = "https://open.feishu.cn";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PAYLOAD: usize = 1024 * 1024;
/// 连接存活超过该时长才把重连计数清零。
const STABLE_CONNECTION: Duration = Duration::from_secs(30);

/// 重连退避：1s 起翻倍，上限 60s（1,2,4,8,16,32,60,60…）。
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(6)).min(60))
}

pub struct Config {
    pub app_id: String,
    pub app_secret: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncomingMessage {
    pub message_id: String,
    pub chat_id: String,
    pub sender_open_id: String,
    /// 发送人的 union_id：同一企业内跨应用一致（open_id 按应用区分），判断「是不是机器人主人」只认它。
    pub sender_union_id: Option<String>,
    pub text: String,
    pub is_group: bool,
    pub mentions_bot: bool,
    /// 飞书消息类型：text / post / image / file / merge_forward 等。
    pub message_type: String,
    /// 被回复（引用）的消息 ID。
    pub parent_id: Option<String>,
    pub root_id: Option<String>,
    pub create_time_ms: i64,
}

pub enum Status {
    Connecting,
    Connected,
    Disconnected(String),
    AppDeleted,
}

pub const APP_DELETED: &str = "飞书应用已被删除";

#[derive(Clone)]
pub struct Client {
    config: Arc<Config>,
    api: String,
    http: Result<HttpClient, &'static str>,
    token: Arc<Mutex<Option<Token>>>,
}

struct Token {
    value: String,
    refresh_at: Instant,
}

#[derive(Deserialize)]
struct ApiResponse {
    code: i64,
}

#[derive(Deserialize)]
struct ReplyResponse {
    code: i64,
    data: Option<ReplyData>,
}

#[derive(Deserialize)]
struct ReplyData {
    message_id: String,
}

#[derive(Deserialize)]
struct ReactionResponse {
    code: i64,
    data: Option<ReactionData>,
}

#[derive(Deserialize)]
struct ReactionData {
    reaction_id: String,
}

fn valid_message_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Deserialize)]
struct TokenResponse {
    code: i64,
    tenant_access_token: Option<String>,
    expire: Option<u64>,
}

#[derive(Deserialize)]
struct BotResponse {
    code: i64,
    bot: Option<Bot>,
}

#[derive(Deserialize)]
struct Bot {
    open_id: String,
    app_name: Option<String>,
}

#[derive(Deserialize)]
struct EndpointResponse {
    code: i64,
    data: Option<Endpoint>,
}

#[derive(Deserialize)]
struct Endpoint {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "ClientConfig")]
    config: Option<SocketConfig>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SocketConfig {
    ping_interval: Option<u64>,
    reconnect_interval: Option<u64>,
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

impl Client {
    pub fn new(config: Config) -> Self {
        Self::with_base_url(config, API)
    }

    /// 指定 HTTP 接口根地址（测试用 mock 服务器；生产用 `new`）。
    pub fn with_base_url(config: Config, api: &str) -> Self {
        let http = HttpClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| "无法初始化飞书 HTTPS 客户端");
        Self {
            config: Arc::new(config),
            api: api.trim_end_matches('/').to_owned(),
            http,
            token: Arc::new(Mutex::new(None)),
        }
    }

    fn http(&self) -> Result<&HttpClient, String> {
        self.http.as_ref().map_err(|error| error.to_string())
    }

    pub async fn probe(&self) -> Result<(), String> {
        self.bot_open_id().await?;
        self.endpoint().await?;
        Ok(())
    }

    async fn tenant_token(&self) -> Result<String, String> {
        let api = &self.api;
        let mut cache = self.token.lock().await;
        if let Some(token) = cache
            .as_ref()
            .filter(|token| Instant::now() < token.refresh_at)
        {
            return Ok(token.value.clone());
        }
        let response: TokenResponse = request_json(
            self.http()?
                .post(format!(
                    "{api}/open-apis/auth/v3/tenant_access_token/internal"
                ))
                .json(&json!({
                    "app_id": self.config.app_id,
                    "app_secret": self.config.app_secret,
                })),
            "获取飞书访问令牌",
        )
        .await?;
        check_code(response.code, "获取飞书访问令牌")?;
        let value = response
            .tenant_access_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "飞书访问令牌响应不完整".to_owned())?;
        let ttl = response
            .expire
            .unwrap_or_default()
            .saturating_sub(60)
            .min(7200);
        *cache = Some(Token {
            value: value.clone(),
            refresh_at: Instant::now() + Duration::from_secs(ttl),
        });
        Ok(value)
    }

    pub async fn bot_name(&self) -> Option<String> {
        let name = self.bot_info().await.ok()?.app_name?;
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_control) { return None; }
        let mut end = name.len().min(120);
        while !name.is_char_boundary(end) { end -= 1; }
        Some(name[..end].to_owned())
    }

    async fn bot_info(&self) -> Result<Bot, String> {
        let api = &self.api;
        let token = self.tenant_token().await?;
        let response: BotResponse = request_json(
            self.http()?
                .get(format!("{api}/open-apis/bot/v3/info"))
                .bearer_auth(token),
            "获取飞书机器人信息",
        )
        .await?;
        check_code(response.code, "获取飞书机器人信息")?;
        response.bot.filter(|bot| !bot.open_id.is_empty()).ok_or_else(|| {
            "无法获取飞书机器人信息，请在飞书应用里开启机器人能力".to_owned()
        })
    }

    async fn bot_open_id(&self) -> Result<String, String> {
        Ok(self.bot_info().await?.open_id)
    }

    async fn endpoint(&self) -> Result<(Url, i32, SocketConfig), String> {
        let api = &self.api;
        let response: EndpointResponse = request_json(
            self.http()?
                .post(format!("{api}/callback/ws/endpoint"))
                .header("locale", "zh")
                .json(&json!({
                    "AppID": self.config.app_id,
                    "AppSecret": self.config.app_secret,
                })),
            "获取飞书长连接地址",
        )
        .await?;
        check_code(response.code, "获取飞书长连接地址")?;
        let endpoint = response
            .data
            .ok_or_else(|| "飞书没有返回长连接地址".to_owned())?;
        let (url, service) = validate_endpoint(&endpoint.url)?;
        Ok((url, service, endpoint.config.unwrap_or_default()))
    }

    /// 回复一条消息，返回新消息的 message_id（供续聊映射用）。
    pub async fn reply(&self, message_id: &str, text: &str) -> Result<Option<String>, String> {
        self.reply_content(message_id, "text", json!({ "text": text }))
            .await
    }

    /// 用消息卡片回复（`msg_type: interactive`，content 是卡片 JSON 的字符串），返回新消息的 message_id。
    /// 卡片内容由调用方生成（见 `review_card::render`，蓝色标题头）。
    pub async fn reply_card(&self, message_id: &str, card: &serde_json::Value) -> Result<Option<String>, String> {
        self.reply_content(message_id, "interactive", card.clone())
            .await
    }

    pub async fn working_reaction(&self, message_id: &str) -> Option<String> {
        let api = &self.api;
        if !valid_message_id(message_id) {
            return None;
        }
        let url = format!("{api}/open-apis/im/v1/messages/{message_id}/reactions");
        let response: ReactionResponse = request_json(
            self.http()
                .ok()?
                .post(url)
                .timeout(Duration::from_secs(3))
                .bearer_auth(self.tenant_token().await.ok()?)
                .json(&json!({ "reaction_type": { "emoji_type": "Typing" } })),
            "添加飞书处理中表情",
        )
        .await
        .ok()?;
        if response.code != 0 {
            eprintln!(
                "beetle: Feishu working reaction rejected (code {})",
                response.code
            );
            return None;
        }
        response.data.map(|data| data.reaction_id)
    }

    pub async fn remove_working_reaction(&self, message_id: &str, reaction_id: &str) {
        let api = &self.api;
        if !valid_message_id(message_id) || reaction_id.is_empty() || reaction_id.len() > 512 {
            return;
        }
        if let (Ok(http), Ok(token)) = (self.http(), self.tenant_token().await) {
            let Ok(mut url) = Url::parse(&format!(
                "{api}/open-apis/im/v1/messages/{message_id}/reactions"
            )) else {
                return;
            };
            if let Ok(mut segments) = url.path_segments_mut() {
                segments.push(reaction_id);
            }
            let _ = http
                .delete(url)
                .timeout(Duration::from_secs(3))
                .bearer_auth(token)
                .send()
                .await;
        }
    }

    async fn reply_content(
        &self,
        message_id: &str,
        kind: &str,
        content: serde_json::Value,
    ) -> Result<Option<String>, String> {
        if !valid_message_id(message_id) {
            return Err("消息 ID 不合法".into());
        }
        let body = json!({
            "msg_type": kind,
            "content": content.to_string(),
        });
        let api = &self.api;
        let url = format!("{api}/open-apis/im/v1/messages/{message_id}/reply");
        self.send_content(&url, body).await
    }

    async fn send_content(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<Option<String>, String> {
        let token = self.tenant_token().await?;
        let mut response: ReplyResponse = request_json(
            self.http()?.post(url).bearer_auth(&token).json(&body),
            "回复飞书消息",
        )
        .await?;

        // Retry only an explicit token rejection to avoid posting duplicate replies.
        if matches!(response.code, 99991663 | 99991664 | 99991668) {
            let mut cache = self.token.lock().await;
            if cache.as_ref().is_some_and(|cached| cached.value == token) {
                *cache = None;
            }
            drop(cache);
            let fresh = self.tenant_token().await?;
            response = request_json(
                self.http()?.post(url).bearer_auth(fresh).json(&body),
                "回复飞书消息",
            )
            .await?;
        }

        check_code(response.code, "回复飞书消息")?;
        Ok(response.data.map(|data| data.message_id))
    }

    pub async fn run(
        &self,
        mut stop: watch::Receiver<bool>,
        on_message: impl Fn(IncomingMessage) -> bool,
        on_status: impl Fn(Status),
    ) {
        let reconnect = async {
            let mut attempt = 0_u32;
            loop {
                on_status(Status::Connecting);
                let result = self.open_socket().await;
                let (error, floor) = match result {
                    Ok((mut socket, service, config, bot_open_id)) => {
                        let connected_at = Instant::now();
                        on_status(Status::Connected);
                        // 服务端下发的重连间隔只作下限，退避仍按 backoff 走。
                        let floor = Duration::from_secs(config.reconnect_interval.unwrap_or(0).min(60));
                        let Err(error) = listen(
                            &mut socket,
                            service,
                            config,
                            &bot_open_id,
                            &self.config.app_id,
                            &on_message,
                        )
                        .await;
                        // 连接稳定存活够久才算恢复；接受后立即断开的不清零。
                        if connected_at.elapsed() >= STABLE_CONNECTION {
                            attempt = 0;
                        }
                        (error.to_owned(), floor)
                    }
                    Err(error) => (error, Duration::ZERO),
                };
                if error == APP_DELETED {
                    on_status(Status::AppDeleted);
                    break;
                }
                let delay = backoff(attempt).max(floor);
                attempt = attempt.saturating_add(1);
                on_status(Status::Disconnected(error));
                sleep(delay).await;
            }
        };
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => {}
            _ = reconnect => {}
        }
    }

    async fn open_socket(&self) -> Result<(Socket, i32, SocketConfig, String), String> {
        let bot = self.bot_open_id().await?;
        let (url, service, config) = self.endpoint().await?;
        let websocket_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_PAYLOAD))
            .max_frame_size(Some(MAX_PAYLOAD));
        let (socket, _) = timeout(
            REQUEST_TIMEOUT,
            connect_async_with_config(url.as_str(), Some(websocket_config), false),
        )
        .await
        .map_err(|_| "飞书长连接握手超时")?
        .map_err(|error| match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                format!(
                    "飞书长连接握手失败（HTTP {}）",
                    response.status().as_u16()
                )
            }
            _ => "飞书长连接握手失败".to_owned(),
        })?;
        Ok((socket, service, config, bot))
    }
}

async fn request_json<T: DeserializeOwned>(
    request: RequestBuilder,
    operation: &str,
) -> Result<T, String> {
    let response = request.send().await.map_err(|error| {
        let reason = if error.is_timeout() {
            "超时"
        } else {
            "请求失败"
        };
        format!("{operation}{reason}")
    })?;
    if !response.status().is_success() {
        return Err(format!(
            "{operation}失败（HTTP {}）",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_| format!("{operation}返回内容无法解析"))
}

fn check_code(code: i64, operation: &str) -> Result<(), String> {
    match code {
        0 => Ok(()),
        10217 => Err(APP_DELETED.into()),
        _ => Err(format!("{operation}失败（飞书错误码 {code}）")),
    }
}

fn validate_endpoint(raw: &str) -> Result<(Url, i32), String> {
    let invalid = || "飞书返回了不可信的长连接地址".to_owned();
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "wss"
        || !url
            .host_str()
            .is_some_and(|host| host.ends_with(".feishu.cn"))
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let service = url
        .query_pairs()
        .find(|(key, _)| key == "service_id")
        .and_then(|(_, value)| value.parse::<i32>().ok())
        .filter(|service| *service > 0)
        .ok_or_else(invalid)?;
    Ok((url, service))
}

// Feishu's pbbp2 frame uses proto2 required fields, including zero-valued ping fields.
#[derive(Clone, PartialEq, ProstMessage)]
struct Frame {
    #[prost(uint64, required, tag = "1")]
    seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    log_id: u64,
    #[prost(int32, required, tag = "3")]
    service: i32,
    #[prost(int32, required, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<FrameHeader>,
    #[prost(string, optional, tag = "6")]
    payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    payload_type: Option<String>,
    #[prost(bytes, optional, tag = "8")]
    payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    log_id_new: Option<String>,
}

#[derive(Clone, PartialEq, ProstMessage)]
struct FrameHeader {
    #[prost(string, required, tag = "1")]
    key: String,
    #[prost(string, required, tag = "2")]
    value: String,
}

impl Frame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
    }

    fn ping(service: i32) -> Self {
        let mut frame = Self {
            service,
            ..Self::default()
        };
        frame.headers.push(FrameHeader {
            key: "type".into(),
            value: "ping".into(),
        });
        frame
    }

    fn acknowledge(&mut self, code: u16, started: Instant) {
        self.headers.push(FrameHeader {
            key: "biz_rt".into(),
            value: started.elapsed().as_millis().to_string(),
        });
        self.payload = Some(
            json!({ "code": code, "headers": null, "data": null })
                .to_string()
                .into_bytes(),
        );
    }
}

async fn send(socket: &mut Socket, message: Message) -> Result<(), &'static str> {
    timeout(WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| "飞书长连接写入超时")?
        .map_err(|_| "飞书长连接写入失败")
}

async fn listen(
    socket: &mut Socket,
    service: i32,
    config: SocketConfig,
    bot: &str,
    app_id: &str,
    on_message: &impl Fn(IncomingMessage) -> bool,
) -> Result<Infallible, &'static str> {
    let mut interval = ping_interval(config.ping_interval.unwrap_or(120));
    let mut ping_at = Instant::now();
    let mut last_received = ping_at;
    let mut fragments = Fragments::default();
    loop {
        tokio::select! {
            _ = sleep_until(last_received + interval * 2 + Duration::from_secs(10)) => {
                return Err("飞书长连接心跳超时");
            }
            _ = sleep_until(ping_at) => {
                send(socket, Message::Binary(Frame::ping(service).encode_to_vec().into())).await?;
                ping_at = Instant::now() + interval;
            }
            message = socket.next() => {
                last_received = Instant::now();
                match message {
                    Some(Ok(Message::Binary(data))) => {
                        let Ok(mut frame) = Frame::decode(data) else { continue };
                        match (frame.method, frame.header("type")) {
                            (0, Some("pong")) => {
                                if let Some(new_interval) = frame.payload.as_deref()
                                    .and_then(|payload| serde_json::from_slice::<SocketConfig>(payload).ok())
                                    .and_then(|config| config.ping_interval)
                                {
                                    interval = ping_interval(new_interval);
                                    ping_at = Instant::now() + interval;
                                }
                            }
                            (1, Some("event")) => {
                                let Some(payload) = fragments.take_payload(&mut frame) else { continue };
                                // Only acknowledge successful admission. The callback must
                                // enqueue immediately so it never holds the socket open.
                                let admitted = parse_event(&payload, bot, app_id)
                                    .is_ok_and(|event| event.is_none_or(on_message));
                                frame.acknowledge(if admitted { 200 } else { 500 }, last_received);
                                send(socket, Message::Binary(frame.encode_to_vec().into())).await?;
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // Tungstenite queues the WebSocket-level pong when it reads a ping.
                        timeout(WRITE_TIMEOUT, socket.flush()).await
                            .map_err(|_| "飞书长连接响应超时")?
                            .map_err(|_| "飞书长连接响应失败")?;
                    }
                    Some(Ok(Message::Close(_))) | None => return Err("飞书长连接已断开"),
                    Some(Err(_)) => return Err("飞书长连接读取失败"),
                    _ => {}
                }
            }
        }
    }
}

fn ping_interval(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(1, 300))
}

#[derive(Default)]
struct Fragments(HashMap<String, Batch>);

struct Batch {
    expires_at: Instant,
    parts: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

impl Fragments {
    fn take_payload(&mut self, frame: &mut Frame) -> Option<Vec<u8>> {
        let sum = frame.header("sum").unwrap_or("1").parse::<usize>().ok()?;
        if sum == 1 {
            return Some(frame.payload.take().unwrap_or_default());
        }
        let seq = frame.header("seq")?.parse::<usize>().ok()?;
        if sum > 16 || seq >= sum {
            return None;
        }
        let message_id = frame
            .header("message_id")
            .filter(|id| !id.is_empty() && id.len() < 256)?
            .to_owned();
        let payload = frame.payload.take().unwrap_or_default();
        let now = Instant::now();
        self.0.retain(|_, batch| batch.expires_at > now);
        if self.0.len() >= 16 && !self.0.contains_key(&message_id) {
            return None;
        }
        let batch = self.0.entry(message_id.clone()).or_insert_with(|| Batch {
            expires_at: now + Duration::from_secs(5),
            parts: vec![None; sum],
            bytes: 0,
        });
        if batch.parts.len() != sum {
            return None;
        }
        let slot = &mut batch.parts[seq];
        if slot.is_none() {
            batch.bytes += payload.len();
            *slot = Some(payload);
        }
        if batch.bytes > MAX_PAYLOAD {
            self.0.remove(&message_id);
            return None;
        }
        if batch.parts.iter().any(Option::is_none) {
            return None;
        }
        self.0
            .remove(&message_id)
            .map(|batch| batch.parts.into_iter().flatten().flatten().collect())
    }
}

#[derive(Deserialize)]
struct EventEnvelope {
    schema: String,
    header: EventHeader,
    event: serde_json::Value,
}

#[derive(Deserialize)]
struct EventHeader {
    event_type: String,
    app_id: String,
    tenant_key: String,
}

#[derive(Deserialize)]
struct MessageEvent {
    sender: Sender,
    message: EventMessage,
}

#[derive(Deserialize)]
struct Sender {
    sender_type: String,
    sender_id: OpenId,
}

#[derive(Deserialize)]
struct OpenId {
    open_id: Option<String>,
    #[serde(default)]
    union_id: Option<String>,
}

#[derive(Deserialize)]
struct EventMessage {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    content: String,
    #[serde(default)]
    mentions: Option<Vec<Mention>>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    root_id: Option<String>,
    /// 飞书以毫秒时间戳字符串给出。
    #[serde(default)]
    create_time: Option<String>,
}

#[derive(Deserialize)]
struct Mention {
    key: String,
    #[serde(default)]
    name: Option<String>,
    id: OpenId,
}

#[derive(Deserialize)]
struct TextContent {
    text: String,
}

fn parse_event(
    payload: &[u8],
    bot: &str,
    app_id: &str,
) -> serde_json::Result<Option<IncomingMessage>> {
    let envelope: EventEnvelope = serde_json::from_slice(payload)?;
    if envelope.schema != "2.0"
        || envelope.header.app_id != app_id
        || envelope.header.event_type != "im.message.receive_v1"
    {
        return Ok(None);
    }
    let event: MessageEvent = serde_json::from_value(envelope.event)?;
    let message = event.message;
    if event.sender.sender_type != "user"
        || !matches!(message.chat_type.as_str(), "p2p" | "group")
        || message.message_id.is_empty()
        || message.chat_id.is_empty()
        || envelope.header.tenant_key.is_empty()
    {
        return Ok(None);
    }
    let sender_union_id = event.sender.sender_id.union_id.clone().filter(|id| !id.is_empty());
    let Some(sender) = event
        .sender
        .sender_id
        .open_id
        .filter(|id| !id.is_empty() && id != bot)
    else {
        return Ok(None);
    };
    let mentions = message.mentions.unwrap_or_default();
    let bot_mention_keys: Vec<&str> = mentions
        .iter()
        .filter(|mention| mention.id.open_id.as_deref() == Some(bot))
        .map(|mention| mention.key.as_str())
        .collect();
    let names: Vec<(&str, &str)> = mentions
        .iter()
        .filter(|mention| mention.id.open_id.as_deref() != Some(bot))
        .filter_map(|mention| Some((mention.key.as_str(), mention.name.as_deref()?)))
        .filter(|(_, name)| !name.is_empty())
        .collect();
    let text = message_text(&message.message_type, &message.content, &bot_mention_keys, &names);
    let non_empty = |id: Option<String>| id.filter(|id| !id.is_empty());
    Ok(Some(IncomingMessage {
        message_id: message.message_id,
        chat_id: message.chat_id,
        sender_open_id: sender,
        sender_union_id,
        text,
        is_group: message.chat_type == "group",
        mentions_bot: !bot_mention_keys.is_empty(),
        message_type: message.message_type,
        parent_id: non_empty(message.parent_id),
        root_id: non_empty(message.root_id),
        create_time_ms: message
            .create_time
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    }))
}

/// 只有 text / post 抽取正文；其余类型给占位文本，让上层仍能识别「有人 @ 了机器人」。
/// 内容 JSON 坏掉时降级为 `[消息]`，不让整条事件解析失败（否则飞书会反复重推）。
fn message_text(
    message_type: &str,
    content: &str,
    bot_mention_keys: &[&str],
    names: &[(&str, &str)],
) -> String {
    match message_type {
        "text" => serde_json::from_str::<TextContent>(content)
            .map(|content| replace_mentions(&content.text, bot_mention_keys, names))
            .unwrap_or_else(|_| "[消息]".into()),
        "post" => serde_json::from_str(content)
            .map(|value| post_text(&value, bot_mention_keys))
            .unwrap_or_else(|_| "[消息]".into()),
        "image" => "[图片]".into(),
        "file" => "[文件]".into(),
        "audio" => "[语音]".into(),
        "media" => "[视频]".into(),
        "sticker" => "[表情]".into(),
        "merge_forward" => "[合并转发]".into(),
        "share_chat" => "[群名片]".into(),
        "share_user" => "[个人名片]".into(),
        "location" => "[位置]".into(),
        "interactive" => "[卡片]".into(),
        _ => "[消息]".into(),
    }
}

/// 富文本：逐段拼出纯文本，去掉 @机器人；其他人的 @ 用名字，图片 / 视频给占位。
fn post_text(content: &serde_json::Value, bot_mention_keys: &[&str]) -> String {
    // 事件里的 post 可能直接是 {title, content}，也可能按语言包一层 {zh_cn: {...}}。
    let body = if content.get("content").is_some() {
        content
    } else {
        content
            .as_object()
            .and_then(|languages| languages.values().find(|value| value.get("content").is_some()))
            .unwrap_or(content)
    };
    let paragraphs = body.get("content").and_then(|value| value.as_array());
    let mut lines = Vec::new();
    // 第一阶段遗留：标题、链接地址、md / 代码块原先被丢掉，Agent 看不到用户贴的文档链接和报错代码
    if let Some(title) = body.get("title").and_then(|t| t.as_str()).map(str::trim).filter(|t| !t.is_empty()) {
        lines.push(title.to_owned());
    }
    for paragraph in paragraphs.into_iter().flatten() {
        let mut line = String::new();
        for element in paragraph.as_array().into_iter().flatten() {
            let field = |name: &str| element.get(name).and_then(|value| value.as_str());
            match field("tag") {
                Some("text") => line.push_str(field("text").unwrap_or_default()),
                Some("a") => {
                    let text = field("text").unwrap_or_default();
                    let href = field("href").unwrap_or_default();
                    line.push_str(text);
                    if !href.is_empty() && href != text {
                        line.push_str(&format!("（{href}）"));
                    }
                }
                Some("md") => line.push_str(field("text").unwrap_or_default()),
                Some("code_block") => {
                    let lang = field("language").unwrap_or_default().to_ascii_lowercase();
                    line.push_str(&format!("\n```{lang}\n{}\n```\n", field("text").unwrap_or_default().trim_end()));
                }
                Some("at") => {
                    let key = field("user_id").unwrap_or_default();
                    if !bot_mention_keys.contains(&key) {
                        line.push('@');
                        line.push_str(field("user_name").filter(|n| !n.is_empty()).unwrap_or(key));
                    }
                }
                Some("img") => line.push_str("[图片]"),
                Some("media") => line.push_str("[视频]"),
                _ => {}
            }
        }
        lines.push(line.trim().to_owned());
    }
    lines.join("\n").trim().to_owned()
}

/// 全文处理 @：机器人的 key 去掉，有名字的其他 key 换成 `@名字`，其余原样保留。
fn replace_mentions(text: &str, bot_keys: &[&str], names: &[(&str, &str)]) -> String {
    let mut remaining = text;
    let mut result = String::with_capacity(text.len());
    while let Some((before, after)) = remaining.split_once("@_user_") {
        result.push_str(before);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let marker_len = "@_user_".len() + digits;
        let marker = &remaining[before.len()..before.len() + marker_len];
        if bot_keys.contains(&marker) {
            // 机器人本身不进正文
        } else if let Some((_, name)) = names.iter().find(|(key, _)| *key == marker) {
            result.push('@');
            result.push_str(name);
        } else {
            result.push_str(marker);
        }
        remaining = &after[digits..];
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_encodes_required_zero_fields() {
        let bytes = Frame::ping(42).encode_to_vec();
        assert_eq!(
            bytes,
            b"\x08\x00\x10\x00\x18\x2a\x20\x00\x2a\x0c\x0a\x04type\x12\x04ping"
        );
    }

    #[test]
    fn accepts_only_feishu_wss_endpoints() {
        let valid = "wss://msg-frontier.feishu.cn/ws/v2?service_id=42&ticket=opaque";
        assert_eq!(validate_endpoint(valid).unwrap().1, 42);
        for invalid in [
            "ws://msg-frontier.feishu.cn/ws/v2?service_id=42",
            "wss://feishu.cn.example.com/ws/v2?service_id=42",
            "wss://msg-frontier.feishu.cn:444/ws/v2?service_id=42",
            "wss://user@msg-frontier.feishu.cn/ws/v2?service_id=42",
            "wss://msg-frontier.feishu.cn/ws/v2?service_id=0",
        ] {
            assert!(validate_endpoint(invalid).is_err());
        }
    }

    #[test]
    fn assembles_fragments_out_of_order_and_preserves_ack_headers() {
        fn part(seq: &str, payload: &[u8]) -> Frame {
            let headers = [("message_id", "event-id"), ("sum", "2"), ("seq", seq)]
                .into_iter()
                .map(|(key, value)| FrameHeader {
                    key: key.into(),
                    value: value.into(),
                })
                .collect();
            Frame {
                headers,
                payload: Some(payload.into()),
                seq_id: 12,
                log_id: 22,
                service: 32,
                method: 1,
                ..Frame::default()
            }
        }

        let mut fragments = Fragments::default();
        assert!(fragments.take_payload(&mut part("1", b"world")).is_none());
        let mut last = part("0", b"hello ");
        assert_eq!(
            fragments.take_payload(&mut last).as_deref(),
            Some(b"hello world".as_slice())
        );
        last.acknowledge(200, Instant::now());
        assert_eq!(last.header("message_id"), Some("event-id"));
        assert_eq!(
            (last.seq_id, last.log_id, last.service, last.method),
            (12, 22, 32, 1)
        );
        let body: serde_json::Value =
            serde_json::from_slice(last.payload.as_deref().unwrap()).unwrap();
        assert_eq!(body["code"], 200);
    }

    #[test]
    fn matches_bot_open_id_and_preserves_literal_command_body() {
        let mut payload = json!({
            "schema": "2.0",
            "header": {
                "app_id": "cli_test",
                "tenant_key": "tenant",
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_type": "user",
                    "sender_id": { "open_id": "ou_user", "union_id": "on_user", "user_id": "u_user" }
                },
                "message": {
                    "message_id": "om_message",
                    "chat_id": "oc_group",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": json!({"text": "@_user_1 /edit @_user_10\n# @_user_1  \n"}).to_string(),
                    "mentions": [
                        { "key": "@_user_1", "id": { "open_id": "ou_bot" } },
                        { "key": "@_user_10", "id": { "open_id": "ou_other" } }
                    ]
                }
            }
        });
        let message = parse_event(&serde_json::to_vec(&payload).unwrap(), "ou_bot", "cli_test")
            .unwrap()
            .unwrap();
        assert!(message.is_group && message.mentions_bot);
        assert_eq!(message.sender_open_id, "ou_user");
        assert_eq!(message.sender_union_id.as_deref(), Some("on_user"));
        assert_eq!(message.text, " /edit @_user_10\n#   \n");

        payload["event"]["sender"]["sender_type"] = json!("app");
        assert!(
            parse_event(&serde_json::to_vec(&payload).unwrap(), "ou_bot", "cli_test")
                .unwrap()
                .is_none()
        );
    }

    fn fixture(raw: &str) -> Option<IncomingMessage> {
        parse_event(raw.as_bytes(), "ou_bot", "cli_test").unwrap()
    }

    #[test]
    fn parses_text_mention_fixture() {
        let m = fixture(include_str!("../../tests/fixtures/feishu/text_mention.json")).unwrap();
        assert!(m.mentions_bot && m.is_group);
        assert_eq!(m.message_type, "text");
        assert_eq!(m.text.trim(), "订单 123 查不到");
        assert_eq!(m.create_time_ms, 1763548800123);
        assert_eq!((m.parent_id, m.root_id), (None, None));
    }

    #[test]
    fn parses_post_with_mention() {
        let m = fixture(include_str!("../../tests/fixtures/feishu/post_mention.json")).unwrap();
        assert!(m.mentions_bot);
        assert_eq!(m.message_type, "post");
        // 链接文字后带上地址（第一阶段遗留：原先 href 被丢掉）
        assert_eq!(m.text, "帮看下这个订单\n详情见 工单（https://example.test/t/1）[图片]");
    }

    #[test]
    fn post_keeps_title_markdown_and_code_block() {
        let content = serde_json::json!({
            "zh_cn": {
                "title": "线上报错",
                "content": [
                    [{"tag": "at", "user_id": "@_user_1"}, {"tag": "md", "text": "**订单 123** 查不到"}],
                    [{"tag": "code_block", "language": "JSON", "text": "{\"code\": 500}\n"}],
                    [{"tag": "a", "text": "https://x.test/a", "href": "https://x.test/a"}, {"tag": "hr"}]
                ]
            }
        });
        let text = post_text(&content, &["@_user_1"]);
        assert_eq!(text, "线上报错\n**订单 123** 查不到\n```json\n{\"code\": 500}\n```\nhttps://x.test/a");
    }

    #[test]
    fn parses_reply_parent_id() {
        let m = fixture(include_str!("../../tests/fixtures/feishu/reply_text.json")).unwrap();
        assert_eq!(m.parent_id.as_deref(), Some("om_parent"));
        assert_eq!(m.root_id.as_deref(), Some("om_root"));
    }

    #[test]
    fn image_message_with_mention_in_group_is_accepted() {
        let m = fixture(include_str!("../../tests/fixtures/feishu/image_mention.json")).unwrap();
        assert!(m.mentions_bot && m.is_group);
        assert_eq!(m.message_type, "image");
        assert_eq!(m.text, "[图片]");
    }

    #[test]
    fn merge_forward_is_accepted() {
        let m = fixture(include_str!("../../tests/fixtures/feishu/merge_forward.json")).unwrap();
        assert_eq!(m.message_type, "merge_forward");
        assert_eq!(m.text, "[合并转发]");
    }

    #[test]
    fn bot_sender_is_ignored() {
        assert!(fixture(include_str!("../../tests/fixtures/feishu/bot_sender.json")).is_none());
    }

    /// 起一个本地 HTTP 服务：令牌接口和回复接口各回一个固定 JSON，并记录收到的请求行。
    async fn mock_feishu(reply_body: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 { return; }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        let Some(head_end) = text.find("\r\n\r\n") else { continue };
                        let length = text[..head_end].lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= head_end + 4 + length { break; }
                    }
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let line = text.lines().next().unwrap_or_default().to_owned();
                    let body = if line.contains("tenant_access_token") {
                        r#"{"code":0,"tenant_access_token":"t-x","expire":7200}"#
                    } else {
                        reply_body
                    };
                    // 记整个请求（请求行打头，后面是头和请求体），测试按需检查请求体
                    log.lock().unwrap().push(text);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        (base, seen)
    }

    #[tokio::test]
    async fn reply_returns_new_message_id() {
        let (base, seen) = mock_feishu(r#"{"code":0,"data":{"message_id":"om_new"}}"#).await;
        let client = Client::with_base_url(
            Config { app_id: "cli_test".into(), app_secret: "secret".into() },
            &base,
        );
        let id = client.reply("om_origin", "收到").await.unwrap();
        assert_eq!(id.as_deref(), Some("om_new"));
        let lines = seen.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l.starts_with("POST /open-apis/im/v1/messages/om_origin/reply")), "{lines:?}");
    }

    #[tokio::test]
    async fn reply_card_posts_interactive_card_body() {
        let (base, seen) = mock_feishu(r#"{"code":0,"data":{"message_id":"om_card"}}"#).await;
        let client = Client::with_base_url(
            Config { app_id: "cli_test".into(), app_secret: "secret".into() },
            &base,
        );
        let card = json!({
            "config": { "wide_screen_mode": true },
            "header": { "template": "blue", "title": { "tag": "plain_text", "content": "代码审查 demo-service #29，3 项发现" } },
            "elements": [{ "tag": "markdown", "content": "【代码审查】demo-service #29" }],
        });
        let id = client.reply_card("om_origin", &card).await.unwrap();
        assert_eq!(id.as_deref(), Some("om_card"));
        let reqs = seen.lock().unwrap().clone();
        let req = reqs
            .iter()
            .find(|r| r.starts_with("POST /open-apis/im/v1/messages/om_origin/reply"))
            .unwrap_or_else(|| panic!("没有发回复请求：{reqs:?}"));
        let body: serde_json::Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["msg_type"], "interactive");
        // content 是卡片 JSON 的字符串，解出来和传入的卡片一致
        let content: serde_json::Value = serde_json::from_str(body["content"].as_str().unwrap()).unwrap();
        assert_eq!(content, card);
        assert_eq!(content["header"]["template"], "blue");
        // 不合法的消息 id 不发请求
        assert!(client.reply_card("om/../x", &card).await.is_err());
    }

    #[test]
    fn backoff_doubles_from_1s_capped_at_60s() {
        let secs: Vec<u64> = (0..9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff(u32::MAX).as_secs(), 60);
    }

    fn text_event(content: &str, mentions: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "2.0",
            "header": { "app_id": "cli_test", "tenant_key": "t", "event_type": "im.message.receive_v1" },
            "event": {
                "sender": { "sender_type": "user", "sender_id": { "open_id": "ou_user" } },
                "message": {
                    "message_id": "om_m", "chat_id": "oc_g", "chat_type": "group",
                    "message_type": "text", "content": content, "mentions": mentions
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn text_multiline_mentions_stripped_and_named() {
        let content = json!({"text": "订单查不到\n@_user_1 帮 @_user_2 看下 @_user_3"}).to_string();
        let mentions = json!([
            { "key": "@_user_1", "id": { "open_id": "ou_bot" }, "name": "查bug" },
            { "key": "@_user_2", "id": { "open_id": "ou_other" }, "name": "李四" },
            { "key": "@_user_3", "id": { "open_id": "ou_x" } }
        ]);
        let m = parse_event(&text_event(&content, mentions), "ou_bot", "cli_test").unwrap().unwrap();
        assert!(m.mentions_bot);
        assert_eq!(m.text, "订单查不到\n 帮 @李四 看下 @_user_3");
    }

    #[test]
    fn malformed_text_content_degrades_to_placeholder() {
        let m = parse_event(&text_event("not json", json!([])), "ou_bot", "cli_test")
            .unwrap()
            .unwrap();
        assert_eq!(m.text, "[消息]");
        let mut payload: serde_json::Value =
            serde_json::from_slice(&text_event("{}", json!([]))).unwrap();
        payload["event"]["message"]["message_type"] = json!("post");
        payload["event"]["message"]["content"] = json!("{broken");
        let m = parse_event(&serde_json::to_vec(&payload).unwrap(), "ou_bot", "cli_test")
            .unwrap()
            .unwrap();
        assert_eq!(m.text, "[消息]");
    }
}
