use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use chrono::NaiveDate;
use reqwest::{Url, cookie::CookieStore};
use rs12306_core::{PassengerId, SeatType, Station};
use serde::{Deserialize, Serialize};
use sm4::{
    Sm4,
    cipher::{Block, BlockEncrypt, KeyInit},
};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RailwayClientError {
    #[error("login requires manual verification")]
    VerificationRequired,
    #[error("session expired")]
    SessionExpired,
    #[error("12306 client is not configured yet")]
    NotConfigured,
    #[error("request failed: {0}")]
    RequestFailed(String),
    #[error("submission result is unknown: {0}")]
    SubmissionUnknown(String),
}

pub type Result<T> = std::result::Result<T, RailwayClientError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsCodeRequest {
    pub username: String,
    pub id_last4: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsLoginRequest {
    pub username: String,
    pub password: String,
    pub sms_code: String,
    pub cookies: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsCodeResult {
    pub message: String,
    pub cookies: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginResult {
    LoggedIn,
    VerificationRequired { url: Option<String> },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginSession {
    pub result: LoginResult,
    pub cookies: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    LoggedOut,
    LoggedIn,
    VerificationRequired,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketQuery {
    pub from: Station,
    pub to: Station,
    pub date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketCandidate {
    pub train_no: String,
    pub from: Station,
    pub to: Station,
    pub date: NaiveDate,
    pub seat_type: SeatType,
    pub remaining: TicketAvailability,
    pub waitlist_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketAvailability {
    Available { count: Option<u32> },
    Limited,
    NoSeatOnly,
    SoldOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitOrderRequest {
    pub train_no: String,
    pub date: NaiveDate,
    pub seat_type: SeatType,
    pub passengers: Vec<PassengerId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitOrderResult {
    pub order_no: String,
    pub pay_deadline_minutes: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealSubmitOrderRequest {
    pub cookies: String,
    pub secret_str: String,
    pub train_date: NaiveDate,
    pub back_train_date: NaiveDate,
    pub from_station_name: String,
    pub to_station_name: String,
    pub seat_type: SeatType,
    pub passenger_names: Vec<String>,
    pub passenger_id_masks: Vec<String>,
    pub choose_seats: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealSubmitOrderResult {
    pub order_no: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderQueueUpdate {
    pub wait_time: Option<i64>,
    pub wait_count: Option<i64>,
    pub order_no: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealSubmitWaitlistRequest {
    pub cookies: String,
    pub secret_str: String,
    pub train_id: String,
    pub seat_type: SeatType,
    pub passenger_names: Vec<String>,
    pub passenger_id_masks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealSubmitWaitlistResult {
    pub standby_no: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailwayPassenger {
    pub name: String,
    pub passenger_type: String,
    pub id_type_code: String,
    pub id_no: String,
    pub mobile_no: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitlistRequest {
    pub train_no: Option<String>,
    pub date: NaiveDate,
    pub seat_types: Vec<SeatType>,
    pub passengers: Vec<PassengerId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitlistResult {
    pub standby_no: String,
    pub queue_position: Option<u32>,
}

#[async_trait]
pub trait RailwayClient: Send + Sync {
    async fn login(&self, request: LoginRequest) -> Result<LoginResult>;
    async fn check_session(&self) -> Result<SessionState>;
    async fn query_tickets(&self, query: TicketQuery) -> Result<Vec<TicketCandidate>>;
    async fn submit_order(&self, request: SubmitOrderRequest) -> Result<SubmitOrderResult>;
    async fn submit_waitlist(&self, request: WaitlistRequest) -> Result<WaitlistResult>;
}

#[derive(Debug, Default)]
pub struct NotConfiguredRailwayClient;

#[async_trait]
impl RailwayClient for NotConfiguredRailwayClient {
    async fn login(&self, _request: LoginRequest) -> Result<LoginResult> {
        Err(RailwayClientError::NotConfigured)
    }

    async fn check_session(&self) -> Result<SessionState> {
        Ok(SessionState::LoggedOut)
    }

    async fn query_tickets(&self, _query: TicketQuery) -> Result<Vec<TicketCandidate>> {
        Err(RailwayClientError::NotConfigured)
    }

    async fn submit_order(&self, _request: SubmitOrderRequest) -> Result<SubmitOrderResult> {
        Err(RailwayClientError::NotConfigured)
    }

    async fn submit_waitlist(&self, _request: WaitlistRequest) -> Result<WaitlistResult> {
        Err(RailwayClientError::NotConfigured)
    }
}

pub async fn login_12306(request: LoginRequest) -> Result<LoginSession> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let client = http_client_builder()
        .cookie_provider(jar.clone())
        .build()
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    client
        .get("https://kyfw.12306.cn/otn/resources/login.html")
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    let password = encrypt_password(&request.password);
    let response = client
        .post("https://kyfw.12306.cn/passport/web/login")
        .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
        .form(&[
            ("username", request.username.as_str()),
            ("password", password.as_str()),
            ("appid", "otn"),
            ("answer", ""),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    let code = payload["result_code"]
        .as_i64()
        .or_else(|| payload["result_code"].as_str()?.parse().ok())
        .unwrap_or(-1);
    let message = payload["result_message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or("login failed")
        .to_string();

    if code != 0 {
        let result = if code == 11 || message.contains("核验") || message.contains("验证码") {
            LoginResult::VerificationRequired {
                url: Some("https://kyfw.12306.cn/otn/resources/login.html".to_string()),
            }
        } else {
            LoginResult::Failed { reason: message }
        };
        return Ok(LoginSession {
            result,
            cookies: cookies_for_12306(&jar),
            username: None,
        });
    }

    let tk = auth_uamtk(&client).await?;
    let username = auth_uamauthclient(&client, &tk).await.ok();
    Ok(LoginSession {
        result: LoginResult::LoggedIn,
        cookies: cookies_for_12306(&jar),
        username,
    })
}

pub async fn request_12306_sms_code(request: SmsCodeRequest) -> Result<SmsCodeResult> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let client = http_client_builder()
        .cookie_provider(jar.clone())
        .build()
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    client
        .get("https://kyfw.12306.cn/otn/resources/login.html")
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    let verification = check_login_verification(&client, &request.username).await?;
    if !matches!(verification, 1 | 3) {
        let message = match verification {
            0 => "当前账号无需短信核验，请直接使用账号密码登录".to_string(),
            2 => "12306 当前要求滑块核验，请改用二维码登录".to_string(),
            code => format!("12306 返回未知核验方式：{code}"),
        };
        return Err(RailwayClientError::RequestFailed(message));
    }

    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/passport/web/getMessageCode")
        .header("origin", "https://kyfw.12306.cn")
        .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
        .header("x-requested-with", "XMLHttpRequest")
        .form(&[
            ("appid", "otn"),
            ("username", request.username.as_str()),
            ("castNum", request.id_last4.as_str()),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if result_code(&payload) != 0 {
        return Err(RailwayClientError::RequestFailed(result_message(&payload)));
    }
    Ok(SmsCodeResult {
        message: result_message(&payload),
        cookies: cookies_for_12306(&jar),
    })
}

pub async fn login_12306_sms(request: SmsLoginRequest) -> Result<LoginSession> {
    let (client, jar) = client_and_jar_with_cookies(request.cookies.as_deref().unwrap_or(""))?;
    let password = encrypt_password(&request.password);
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/passport/web/login")
        .header("origin", "https://kyfw.12306.cn")
        .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
        .header("x-requested-with", "XMLHttpRequest")
        .form(&[
            ("sessionId", ""),
            ("sig", ""),
            ("if_check_slide_passcode_token", ""),
            ("scene", ""),
            ("checkMode", "0"),
            ("randCode", request.sms_code.as_str()),
            ("username", request.username.as_str()),
            ("password", password.as_str()),
            ("appid", "otn"),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    if result_code(&payload) != 0 {
        return Ok(LoginSession {
            result: LoginResult::Failed {
                reason: result_message(&payload),
            },
            cookies: cookies_for_12306(&jar),
            username: None,
        });
    }

    let tk = auth_uamtk(&client).await?;
    let username = auth_uamauthclient(&client, &tk).await.ok();
    Ok(LoginSession {
        result: LoginResult::LoggedIn,
        cookies: cookies_for_12306(&jar),
        username,
    })
}

async fn check_login_verification(client: &reqwest::Client, username: &str) -> Result<i64> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/passport/web/checkLoginVerify")
        .header("origin", "https://kyfw.12306.cn")
        .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
        .header("x-requested-with", "XMLHttpRequest")
        .form(&[("username", username), ("appid", "otn")])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if result_code(&payload) != 0 {
        return Err(RailwayClientError::RequestFailed(result_message(&payload)));
    }
    payload["login_check_code"]
        .as_i64()
        .or_else(|| payload["login_check_code"].as_str()?.parse().ok())
        .ok_or_else(|| {
            RailwayClientError::RequestFailed("missing login verification mode".to_string())
        })
}

fn encrypt_password(password: &str) -> String {
    sm4_ecb_base64(password, b"tiekeyuankp12306")
}

fn sm4_ecb_base64(plaintext: &str, key: &[u8; 16]) -> String {
    let padding = 16 - plaintext.len() % 16;
    let mut bytes = plaintext.as_bytes().to_vec();
    bytes.resize(bytes.len() + padding, padding as u8);

    let cipher = Sm4::new_from_slice(key).expect("SM4 key has a fixed valid length");
    for chunk in bytes.chunks_exact_mut(16) {
        cipher.encrypt_block(Block::<Sm4>::from_mut_slice(chunk));
    }
    format!("@{}", general_purpose::STANDARD.encode(bytes))
}

pub async fn login_12306_qr<F>(
    qr_image_path: &Path,
    timeout: Duration,
    mut on_qr_refreshed: F,
) -> Result<LoginSession>
where
    F: FnMut(&Path, &[u8]),
{
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let client = http_client_builder()
        .cookie_provider(jar.clone())
        .build()
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    client
        .get("https://kyfw.12306.cn/otn/resources/login.html")
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

    let started_at = Instant::now();
    let mut uuid = create_login_qr(&client, qr_image_path, &mut on_qr_refreshed).await?;
    loop {
        if started_at.elapsed() > timeout {
            return Err(RailwayClientError::RequestFailed(
                "qr login timed out".to_string(),
            ));
        }

        let rail_device_id = cookie_value(&jar, "RAIL_DEVICEID").unwrap_or_default();
        let rail_expiration = cookie_value(&jar, "RAIL_EXPIRATION").unwrap_or_default();
        let payload: serde_json::Value = client
            .post("https://kyfw.12306.cn/passport/web/checkqr")
            .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
            .form(&[
                ("RAIL_DEVICEID", rail_device_id.as_str()),
                ("RAIL_EXPIRATION", rail_expiration.as_str()),
                ("uuid", uuid.as_str()),
                ("appid", "otn"),
            ])
            .send()
            .await
            .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
            .json()
            .await
            .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;

        match result_code(&payload) {
            0 | 1 => tokio::time::sleep(Duration::from_secs(2)).await,
            2 => {
                client
                    .get("https://kyfw.12306.cn/otn/login/userLogin")
                    .send()
                    .await
                    .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
                let tk = auth_uamtk(&client).await?;
                let username = auth_uamauthclient(&client, &tk).await.ok();
                client
                    .get("https://kyfw.12306.cn/otn/login/userLogin")
                    .send()
                    .await
                    .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
                return Ok(LoginSession {
                    result: LoginResult::LoggedIn,
                    cookies: cookies_for_12306(&jar),
                    username,
                });
            }
            3 => {
                uuid = create_login_qr(&client, qr_image_path, &mut on_qr_refreshed).await?;
            }
            _ => return Err(RailwayClientError::RequestFailed(result_message(&payload))),
        }
    }
}

async fn create_login_qr<F>(
    client: &reqwest::Client,
    qr_image_path: &Path,
    on_qr_refreshed: &mut F,
) -> Result<String>
where
    F: FnMut(&Path, &[u8]),
{
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/passport/web/create-qr64")
        .header("referer", "https://kyfw.12306.cn/otn/resources/login.html")
        .form(&[("appid", "otn")])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if result_code(&payload) != 0 {
        return Err(RailwayClientError::RequestFailed(result_message(&payload)));
    }

    let uuid = payload["uuid"]
        .as_str()
        .ok_or_else(|| RailwayClientError::RequestFailed("missing qr uuid".to_string()))?
        .to_string();
    let image = payload["image"]
        .as_str()
        .ok_or_else(|| RailwayClientError::RequestFailed("missing qr image".to_string()))?;
    let image_bytes = general_purpose::STANDARD
        .decode(image)
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    std::fs::write(qr_image_path, &image_bytes)
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    secure_private_file(qr_image_path)?;
    on_qr_refreshed(qr_image_path, &image_bytes);
    Ok(uuid)
}

pub async fn submit_12306_order(request: RealSubmitOrderRequest) -> Result<RealSubmitOrderResult> {
    submit_12306_order_with_queue_updates(request, |_| {}).await
}

pub async fn submit_12306_order_with_queue_updates<F>(
    request: RealSubmitOrderRequest,
    mut on_queue_update: F,
) -> Result<RealSubmitOrderResult>
where
    F: FnMut(OrderQueueUpdate),
{
    let client = client_with_cookies(&request.cookies)?;

    let passengers = fetch_order_passengers(
        &client,
        &request.passenger_names,
        &request.passenger_id_masks,
    )
    .await?;
    submit_order_request(&client, &request).await?;
    let init = init_order_page(&client).await?;
    let passenger_ticket_str = passenger_ticket_str(request.seat_type, &passengers);
    let old_passenger_str = old_passenger_str(&passengers);

    check_order_info(
        &client,
        &init.token,
        &passenger_ticket_str,
        &old_passenger_str,
    )
    .await?;
    get_queue_count(&client, &init, request.seat_type, request.train_date).await?;
    confirm_single_for_queue(
        &client,
        &init,
        &passenger_ticket_str,
        &old_passenger_str,
        request.choose_seats.as_deref().unwrap_or_default(),
    )
    .await?;
    Ok(RealSubmitOrderResult {
        order_no: query_order_wait_time(&client, &init.token, &mut on_queue_update).await?,
    })
}

pub async fn submit_12306_waitlist(
    request: RealSubmitWaitlistRequest,
) -> Result<RealSubmitWaitlistResult> {
    let client = client_with_cookies(&request.cookies)?;
    let passengers = fetch_order_passengers(
        &client,
        &request.passenger_names,
        &request.passenger_id_masks,
    )
    .await?;
    let secret_with_seat = format!("{}#{}|", request.secret_str, seat_code(request.seat_type));

    let face = post_json_form(
        &client,
        "https://kyfw.12306.cn/otn/afterNate/chechFace",
        &[("secretList", secret_with_seat.as_str()), ("_json_att", "")],
    )
    .await?;
    ensure_status(&face)?;
    if !json_truthy(&face["data"]["face_flag"]) {
        return Err(RailwayClientError::VerificationRequired);
    }

    let success_secret = secret_with_seat.trim_end_matches('|');
    let rate = post_json_form(
        &client,
        "https://kyfw.12306.cn/otn/afterNate/getSuccessRate",
        &[("successSecret", success_secret), ("_json_att", "")],
    )
    .await?;
    ensure_status(&rate)?;
    if rate["data"]["flag"]
        .as_array()
        .is_none_or(|flags| flags.is_empty())
    {
        return Err(RailwayClientError::RequestFailed(result_message(&rate)));
    }

    let prepared = post_json_form(
        &client,
        "https://kyfw.12306.cn/otn/afterNate/submitOrderRequest",
        &[("secretList", secret_with_seat.as_str()), ("_json_att", "")],
    )
    .await?;
    ensure_status(&prepared)?;
    if !json_truthy(&prepared["data"]["flag"]) {
        return Err(RailwayClientError::RequestFailed(result_message(&prepared)));
    }

    let init = post_json_form(
        &client,
        "https://kyfw.12306.cn/otn/afterNate/passengerInitApi",
        &[("_json_att", "")],
    )
    .await?;
    ensure_status(&init)?;
    let date = init["data"]["jzdhDateE"]
        .as_str()
        .ok_or_else(|| RailwayClientError::RequestFailed(result_message(&init)))?;
    let time = init["data"]["jzdhHourE"]
        .as_str()
        .ok_or_else(|| RailwayClientError::RequestFailed(result_message(&init)))?;
    let jz_param = format!("{}#{}", date, time.replace(':', "#"));
    let passenger_info = waitlist_passenger_info(&passengers);
    let hb_train = format!("{},{}#", request.train_id, seat_code(request.seat_type));
    let confirmed = post_json_form(
        &client,
        "https://kyfw.12306.cn/otn/afterNate/confirmHB",
        &[
            ("passengerInfo", passenger_info.as_str()),
            ("jzParam", jz_param.as_str()),
            ("hbTrain", hb_train.as_str()),
            ("lkParam", ""),
        ],
    )
    .await?;
    ensure_status(&confirmed)?;
    if !json_truthy(&confirmed["data"]["flag"]) {
        return Err(RailwayClientError::RequestFailed(result_message(
            &confirmed,
        )));
    }

    for _ in 0..10 {
        let queued = post_json_form(
            &client,
            "https://kyfw.12306.cn/otn/afterNate/queryQueue",
            &[("_json_att", "")],
        )
        .await?;
        if queued["status"].as_bool().unwrap_or(false) {
            let standby_no = queued["data"]["orderId"]
                .as_str()
                .or_else(|| queued["data"]["reserve_no"].as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            return Ok(RealSubmitWaitlistResult { standby_no });
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(RailwayClientError::SubmissionUnknown(
        "waitlist queue timed out; check official 12306 before retrying".to_string(),
    ))
}

pub async fn list_12306_passengers(cookies: &str) -> Result<Vec<RailwayPassenger>> {
    let client = client_with_cookies(cookies)?;
    fetch_passenger_dtos(&client)
        .await?
        .into_iter()
        .map(|passenger| {
            Ok(RailwayPassenger {
                name: passenger["passenger_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                passenger_type: passenger["passenger_type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                id_type_code: passenger["passenger_id_type_code"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                id_no: passenger["passenger_id_no"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                mobile_no: passenger["mobile_no"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

async fn submit_order_request(
    client: &reqwest::Client,
    request: &RealSubmitOrderRequest,
) -> Result<()> {
    let secret_str = percent_decode(&request.secret_str)?;
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/leftTicket/submitOrderRequest")
        .header("referer", "https://kyfw.12306.cn/otn/leftTicket/init")
        .form(&[
            ("secretStr", secret_str.as_str()),
            ("train_date", &request.train_date.to_string()),
            ("back_train_date", &request.back_train_date.to_string()),
            ("tour_flag", "dc"),
            ("purpose_codes", "ADULT"),
            (
                "query_from_station_name",
                request.from_station_name.as_str(),
            ),
            ("query_to_station_name", request.to_station_name.as_str()),
            ("undefined", ""),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    let data = payload["data"].as_str().unwrap_or_default();
    if data == "0" || data == "N" {
        return Ok(());
    }
    Err(RailwayClientError::RequestFailed(result_message(&payload)))
}

#[derive(Debug)]
struct OrderInit {
    token: String,
    ticket: serde_json::Value,
}

async fn init_order_page(client: &reqwest::Client) -> Result<OrderInit> {
    let html = client
        .post("https://kyfw.12306.cn/otn/confirmPassenger/initDc")
        .header("referer", "https://kyfw.12306.cn/otn/leftTicket/init")
        .form(&[("_json_att", "")])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .text()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if html.contains("系统忙") {
        return Err(RailwayClientError::RequestFailed(
            "12306 system busy".to_string(),
        ));
    }
    if html.contains("/otn/resources/login.html")
        || html.contains("登录") && !html.contains("globalRepeatSubmitToken")
    {
        return Err(RailwayClientError::SessionExpired);
    }
    let token = between(&html, "var globalRepeatSubmitToken = '", "'").ok_or_else(|| {
        RailwayClientError::RequestFailed("missing repeat submit token".to_string())
    })?;
    let ticket = js_object_after(&html, "var ticketInfoForPassengerForm")
        .and_then(|object| serde_json::from_str(&object.replace('\'', "\"")).ok())
        .ok_or_else(|| RailwayClientError::RequestFailed("missing passenger form".to_string()))?;
    Ok(OrderInit { token, ticket })
}

#[derive(Debug)]
struct OrderPassenger {
    name: String,
    passenger_type: String,
    id_type_code: String,
    id_no: String,
    mobile_no: String,
    all_enc_str: String,
}

async fn fetch_order_passengers(
    client: &reqwest::Client,
    passenger_names: &[String],
    passenger_id_masks: &[String],
) -> Result<Vec<OrderPassenger>> {
    if passenger_names.len() != passenger_id_masks.len() {
        return Err(RailwayClientError::RequestFailed(
            "passenger names and ID masks do not match".to_string(),
        ));
    }
    let normal = fetch_passenger_dtos(client).await?;
    passenger_names
        .iter()
        .zip(passenger_id_masks)
        .map(|(name, id_mask)| {
            let passenger = normal
                .iter()
                .find(|passenger| {
                    passenger["passenger_name"].as_str() == Some(name.as_str())
                        && passenger["passenger_id_no"]
                            .as_str()
                            .is_some_and(|id_no| masked_value_matches(id_mask, id_no))
                })
                .ok_or_else(|| {
                    RailwayClientError::RequestFailed(format!(
                        "passenger `{name}` with ID `{id_mask}` not found in 12306 contacts"
                    ))
                })?;
            Ok(OrderPassenger {
                name: passenger["passenger_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                passenger_type: passenger["passenger_type"]
                    .as_str()
                    .unwrap_or("1")
                    .to_string(),
                id_type_code: passenger["passenger_id_type_code"]
                    .as_str()
                    .unwrap_or("1")
                    .to_string(),
                id_no: passenger["passenger_id_no"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                mobile_no: passenger["mobile_no"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                all_enc_str: passenger["allEncStr"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

async fn fetch_passenger_dtos(client: &reqwest::Client) -> Result<Vec<serde_json::Value>> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/confirmPassenger/getPassengerDTOs")
        .header(
            "referer",
            "https://kyfw.12306.cn/otn/confirmPassenger/initDc",
        )
        .header("x-requested-with", "XMLHttpRequest")
        .form(&[("_json_att", "")])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if payload["isRelogin"].as_str() == Some("Y") || result_message(&payload).contains("登录") {
        return Err(RailwayClientError::SessionExpired);
    }
    let normal = payload["data"]["normal_passengers"]
        .as_array()
        .ok_or_else(|| RailwayClientError::RequestFailed(result_message(&payload)))?;
    Ok(normal.clone())
}

async fn post_json_form(
    client: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<serde_json::Value> {
    client
        .post(url)
        .header("referer", "https://kyfw.12306.cn/otn/leftTicket/init")
        .header("origin", "https://kyfw.12306.cn")
        .header("x-requested-with", "XMLHttpRequest")
        .form(form)
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))
}

fn ensure_status(payload: &serde_json::Value) -> Result<()> {
    if payload["status"].as_bool().unwrap_or(false) {
        Ok(())
    } else if result_message(payload).contains("登录") {
        Err(RailwayClientError::SessionExpired)
    } else {
        Err(RailwayClientError::RequestFailed(result_message(payload)))
    }
}

fn json_truthy(value: &serde_json::Value) -> bool {
    value.as_bool().unwrap_or(false)
        || value.as_i64().is_some_and(|value| value != 0)
        || value
            .as_str()
            .is_some_and(|value| matches!(value, "1" | "Y" | "true"))
}

fn waitlist_passenger_info(passengers: &[OrderPassenger]) -> String {
    passengers
        .iter()
        .map(|passenger| {
            format!(
                "{}#{}#{}#{}#{};",
                passenger.passenger_type,
                passenger.name,
                passenger.id_type_code,
                passenger.id_no,
                passenger.all_enc_str
            )
        })
        .collect()
}

async fn check_order_info(
    client: &reqwest::Client,
    token: &str,
    passenger_ticket_str: &str,
    old_passenger_str: &str,
) -> Result<()> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/confirmPassenger/checkOrderInfo")
        .form(&[
            ("cancel_flag", "2"),
            ("bed_level_order_num", "000000000000000000000000000000"),
            ("passengerTicketStr", passenger_ticket_str),
            ("oldPassengerStr", old_passenger_str),
            ("tour_flag", "dc"),
            ("randCode", ""),
            ("whatsSelect", "1"),
            ("_json_att", ""),
            ("REPEAT_SUBMIT_TOKEN", token),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if payload["data"]["ifShowPassCode"].as_str() == Some("Y") {
        return Err(RailwayClientError::VerificationRequired);
    }
    if payload["data"]["submitStatus"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(RailwayClientError::RequestFailed(result_message(&payload)))
    }
}

async fn get_queue_count(
    client: &reqwest::Client,
    init: &OrderInit,
    seat_type: SeatType,
    train_date: NaiveDate,
) -> Result<()> {
    let dto = &init.ticket["queryLeftTicketRequestDTO"];
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/confirmPassenger/getQueueCount")
        .form(&[
            ("train_date", train_date_for_queue(train_date).as_str()),
            ("train_no", dto["train_no"].as_str().unwrap_or_default()),
            (
                "stationTrainCode",
                dto["station_train_code"].as_str().unwrap_or_default(),
            ),
            ("seatType", seat_code(seat_type)),
            (
                "fromStationTelecode",
                dto["from_station"].as_str().unwrap_or_default(),
            ),
            (
                "toStationTelecode",
                dto["to_station"].as_str().unwrap_or_default(),
            ),
            (
                "leftTicket",
                init.ticket["leftTicketStr"].as_str().unwrap_or_default(),
            ),
            (
                "purpose_codes",
                init.ticket["purpose_codes"].as_str().unwrap_or("00"),
            ),
            (
                "train_location",
                init.ticket["train_location"].as_str().unwrap_or_default(),
            ),
            ("_json_att", ""),
            ("REPEAT_SUBMIT_TOKEN", init.token.as_str()),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if payload["status"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(RailwayClientError::RequestFailed(result_message(&payload)))
    }
}

async fn confirm_single_for_queue(
    client: &reqwest::Client,
    init: &OrderInit,
    passenger_ticket_str: &str,
    old_passenger_str: &str,
    choose_seats: &str,
) -> Result<()> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/confirmPassenger/confirmSingleForQueue")
        .form(&[
            ("passengerTicketStr", passenger_ticket_str),
            ("oldPassengerStr", old_passenger_str),
            ("randCode", ""),
            (
                "purpose_codes",
                init.ticket["purpose_codes"].as_str().unwrap_or("00"),
            ),
            (
                "key_check_isChange",
                init.ticket["key_check_isChange"]
                    .as_str()
                    .unwrap_or_default(),
            ),
            (
                "leftTicketStr",
                init.ticket["leftTicketStr"].as_str().unwrap_or_default(),
            ),
            (
                "train_location",
                init.ticket["train_location"].as_str().unwrap_or_default(),
            ),
            ("choose_seats", choose_seats),
            ("seatDetailType", "000"),
            ("whatsSelect", "1"),
            ("roomType", "00"),
            ("dwAll", "N"),
            ("_json_att", ""),
            ("REPEAT_SUBMIT_TOKEN", init.token.as_str()),
        ])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    if payload["data"]["submitStatus"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(RailwayClientError::RequestFailed(result_message(&payload)))
    }
}

async fn query_order_wait_time<F>(
    client: &reqwest::Client,
    token: &str,
    on_queue_update: &mut F,
) -> Result<String>
where
    F: FnMut(OrderQueueUpdate),
{
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60) {
        let payload: serde_json::Value = client
            .get("https://kyfw.12306.cn/otn/confirmPassenger/queryOrderWaitTime")
            .query(&[
                ("random", unix_millis().to_string()),
                ("tourFlag", "dc".to_string()),
                ("_json_att", "".to_string()),
                ("REPEAT_SUBMIT_TOKEN", token.to_string()),
            ])
            .send()
            .await
            .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
            .json()
            .await
            .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
        let outcome = queue_outcome(&payload);
        let order_no = payload["data"]["orderId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let wait_time = json_i64(&payload["data"]["waitTime"]);
        let wait_count =
            json_i64(&payload["data"]["waitCount"]).or_else(|| json_i64(&payload["data"]["count"]));
        on_queue_update(OrderQueueUpdate {
            wait_time,
            wait_count,
            order_no: order_no.clone(),
        });
        if let Some(order_no) = outcome? {
            return Ok(order_no);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Err(RailwayClientError::SubmissionUnknown(
        "order queue timed out after 60 seconds; check official 12306 before retrying".to_string(),
    ))
}

fn queue_outcome(payload: &serde_json::Value) -> Result<Option<String>> {
    if let Some(order_no) = payload["data"]["orderId"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(order_no.to_string()));
    }
    if json_i64(&payload["data"]["waitTime"]) == Some(-1) {
        return Err(RailwayClientError::RequestFailed(format!(
            "order queue ended without an order: {}",
            result_message(payload)
        )));
    }
    Ok(None)
}

fn passenger_ticket_str(seat_type: SeatType, passengers: &[OrderPassenger]) -> String {
    passengers
        .iter()
        .map(|passenger| {
            format!(
                "{},0,{},{},{},{},{},N,{}",
                seat_code(seat_type),
                passenger.passenger_type,
                passenger.name,
                passenger.id_type_code,
                passenger.id_no,
                passenger.mobile_no,
                passenger.all_enc_str
            )
        })
        .collect::<Vec<_>>()
        .join("_")
}

fn old_passenger_str(passengers: &[OrderPassenger]) -> String {
    passengers
        .iter()
        .map(|passenger| {
            format!(
                "{},{},{},{}_",
                passenger.name, passenger.id_type_code, passenger.id_no, passenger.passenger_type
            )
        })
        .collect()
}

pub fn seat_code(seat_type: SeatType) -> &'static str {
    match seat_type {
        SeatType::Business => "9",
        SeatType::FirstClass => "M",
        SeatType::SecondClass => "O",
        SeatType::SoftSleeper => "4",
        SeatType::HardSleeper => "3",
        SeatType::HardSeat | SeatType::NoSeat => "1",
    }
}

fn train_date_for_queue(date: NaiveDate) -> String {
    format!(
        "{} 00:00:00 GMT+0800 (China Standard Time)",
        date.format("%a %b %d %Y")
    )
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn between(text: &str, start: &str, end: &str) -> Option<String> {
    let from = text.find(start)? + start.len();
    let to = text[from..].find(end)? + from;
    Some(text[from..to].to_string())
}

fn js_object_after(text: &str, marker: &str) -> Option<String> {
    let start = text[text.find(marker)?..].find('{')? + text.find(marker)?;
    let mut depth = 0;
    let mut in_quote = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '\'' {
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(text[start..=start + offset].to_string());
            }
        }
    }
    None
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|error| RailwayClientError::RequestFailed(error.to_string()))
}

fn client_with_cookies(cookies: &str) -> Result<reqwest::Client> {
    Ok(client_and_jar_with_cookies(cookies)?.0)
}

fn client_and_jar_with_cookies(
    cookies: &str,
) -> Result<(reqwest::Client, Arc<reqwest::cookie::Jar>)> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    let url = Url::parse("https://kyfw.12306.cn/")
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    for cookie in cookies.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        jar.add_cookie_str(cookie, &url);
    }
    let client = http_client_builder()
        .cookie_provider(jar.clone())
        .build()
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    Ok((client, jar))
}

fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
}

fn masked_value_matches(mask: &str, value: &str) -> bool {
    let mask = mask.trim();
    let mask_chars: Vec<_> = mask.chars().collect();
    let value_chars: Vec<_> = value.chars().collect();
    mask_chars.len() == value_chars.len()
        && mask_chars
            .iter()
            .zip(value_chars)
            .all(|(expected, actual)| *expected == '*' || *expected == actual)
}

#[cfg(unix)]
fn secure_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))
}

#[cfg(not(unix))]
fn secure_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

async fn auth_uamtk(client: &reqwest::Client) -> Result<String> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/passport/web/auth/uamtk")
        .header(
            "referer",
            "https://kyfw.12306.cn/otn/passport?redirect=/otn/login/userLogin",
        )
        .header("origin", "https://kyfw.12306.cn")
        .form(&[("appid", "otn")])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    payload["newapptk"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| RailwayClientError::RequestFailed("missing newapptk".to_string()))
}

async fn auth_uamauthclient(client: &reqwest::Client, tk: &str) -> Result<String> {
    let payload: serde_json::Value = client
        .post("https://kyfw.12306.cn/otn/uamauthclient")
        .form(&[("tk", tk)])
        .send()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?
        .json()
        .await
        .map_err(|error| RailwayClientError::RequestFailed(error.to_string()))?;
    payload["username"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| RailwayClientError::RequestFailed("missing username".to_string()))
}

fn result_code(payload: &serde_json::Value) -> i64 {
    json_i64(&payload["result_code"]).unwrap_or(-1)
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn result_message(payload: &serde_json::Value) -> String {
    if let Some(message) = payload["result_message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .or_else(|| payload["data"]["errMsg"].as_str())
        .or_else(|| payload["data"]["exMsg"].as_str())
    {
        return message.to_string();
    }
    if let Some(messages) = payload["messages"].as_array() {
        let message = messages
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        if !message.is_empty() {
            return message;
        }
    }
    if let Some(message) = payload["messages"].as_str() {
        return message.to_string();
    }
    if let Some(message) = payload["validateMessages"].as_str() {
        return message.to_string();
    }
    if payload["validateMessages"].is_object() || payload["validateMessages"].is_array() {
        return payload["validateMessages"].to_string();
    }
    "request failed".to_string()
}

fn cookie_value(jar: &reqwest::cookie::Jar, name: &str) -> Option<String> {
    let url = Url::parse("https://kyfw.12306.cn/").ok()?;
    let cookies = jar.cookies(&url)?;
    let cookies = cookies.to_str().ok()?;
    cookies.split(';').find_map(|cookie| {
        let (cookie_name, cookie_value) = cookie.trim().split_once('=')?;
        (cookie_name == name).then(|| cookie_value.to_string())
    })
}

fn cookies_for_12306(jar: &reqwest::cookie::Jar) -> Option<String> {
    let url = Url::parse("https://kyfw.12306.cn/otn/confirmPassenger/getPassengerDTOs").ok()?;
    jar.cookies(&url)
        .and_then(|cookies| cookies.to_str().ok().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_decodes_secret_str_parts() {
        assert_eq!(percent_decode("a%2Bb%3D%3D%0A").unwrap(), "a+b==\n");
    }

    #[test]
    fn encrypts_password_like_the_official_login_page() {
        assert_eq!(
            sm4_ecb_base64("1234", b"1234567890123456"),
            "@woPrxebr8Xvyo1qG8QxAUA=="
        );
    }

    #[test]
    fn matches_masked_passenger_ids() {
        assert!(masked_value_matches(
            "6223***********21X",
            "62231234567890121X"
        ));
        assert!(!masked_value_matches(
            "6223***********21X",
            "62231234567890122X"
        ));
    }

    #[test]
    fn rejects_queue_completion_without_order_id() {
        let payload = serde_json::json!({"data": {"waitTime": -1, "msg": "failed"}});
        assert!(queue_outcome(&payload).is_err());

        let payload = serde_json::json!({"data": {"waitTime": 0, "orderId": "E123"}});
        assert_eq!(queue_outcome(&payload).unwrap(), Some("E123".to_string()));
    }

    #[test]
    fn builds_waitlist_passenger_info() {
        let passenger = OrderPassenger {
            name: "张三".to_string(),
            passenger_type: "1".to_string(),
            id_type_code: "1".to_string(),
            id_no: "310101199001011234".to_string(),
            mobile_no: String::new(),
            all_enc_str: "encrypted".to_string(),
        };
        assert_eq!(
            waitlist_passenger_info(&[passenger]),
            "1#张三#1#310101199001011234#encrypted;"
        );
    }
}
