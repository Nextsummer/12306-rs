use std::{
    collections::HashSet,
    io::{self, Cursor, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, bail};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use clap::{Args, Parser, Subcommand};
use rs12306_client_12306::{
    LoginRequest, LoginResult, OrderQueueUpdate, RailwayClientError, RealSubmitOrderRequest,
    RealSubmitWaitlistRequest, SmsCodeRequest, SmsLoginRequest, list_12306_passengers, login_12306,
    login_12306_qr, login_12306_sms, request_12306_sms_code, seat_code,
    submit_12306_order_with_queue_updates, submit_12306_waitlist,
};
use rs12306_core::{
    DEFAULT_QUERY_INTERVAL_MS, NewTicketTask, NewTrainPolicy, Passenger, PassengerId,
    PassengerType, SeatType, Station, TaskStatus, TrainFilter, TrainFilterKind,
};
use rs12306_server::{ServerConfig, query_12306_tickets, serve};
use rs12306_storage::{DEFAULT_DATABASE_PATH, Database};
use uuid::Uuid;

mod notification;

#[derive(Debug, Parser)]
#[command(name = "12306-rs")]
#[command(about = "12306 余票查询、购票任务和通知服务")]
#[command(arg_required_else_help = true)]
#[command(
    after_help = "常用示例:\n  12306-rs login --qr\n  12306-rs query --from 上海 --to 嘉兴 --date <YYYY-MM-DD>\n  12306-rs passenger 12306-list\n  12306-rs notify types\n\n查看子命令帮助:\n  12306-rs <命令> --help"
)]
struct Cli {
    #[arg(
        long,
        env = "RS12306_DATABASE",
        default_value = DEFAULT_DATABASE_PATH,
        help = "SQLite 数据库路径"
    )]
    database: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动 Web UI 和 HTTP API 服务
    Serve(ServeArgs),
    /// 登录 12306，支持二维码、密码和短信验证
    Login(LoginArgs),
    /// 清除当前本地登录会话
    Logout,
    /// 查看数据库、任务数量和真实登录状态
    Status,
    /// 查询指定线路和日期的真实余票
    Query(QueryArgs),
    /// 选择车次并提交一次真实普通订单
    Buy(BuyArgs),
    /// 查看、同步或管理乘车人
    Passenger {
        #[command(subcommand)]
        command: PassengerCommand,
    },
    /// 创建和运行前台抢票任务
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// 配置和测试全局通知方式
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// 监听地址
    #[arg(long, env = "RS12306_HOST", default_value = "127.0.0.1")]
    host: String,

    /// 监听端口
    #[arg(long, env = "RS12306_PORT", default_value_t = 12306)]
    port: u16,
}

#[derive(Debug, Args)]
struct LoginArgs {
    /// 12306 用户名、手机号或邮箱
    #[arg(long)]
    username: Option<String>,

    /// 12306 密码；省略时使用隐藏式输入
    #[arg(long, env = "RS12306_PASSWORD", hide_env_values = true)]
    password: Option<String>,

    /// 绑定证件号后 4 位，用于申请短信验证码
    #[arg(long)]
    id_last4: Option<String>,

    /// 收到的短信验证码
    #[arg(long)]
    sms_code: Option<String>,

    /// 使用 12306 App 扫码登录
    #[arg(long)]
    qr: bool,

    /// 二维码 PNG 备用保存路径
    #[arg(long, default_value = "./12306-login-qr.png")]
    qr_path: PathBuf,

    /// 二维码登录最长等待秒数
    #[arg(long, default_value_t = 600)]
    qr_timeout_seconds: u64,

    /// 仅限本地开发：直接标记会话已验证
    #[arg(long)]
    verified: bool,
}

#[derive(Debug, Args)]
struct QueryArgs {
    /// 出发站中文名、拼音或车站代码
    #[arg(long)]
    from: String,

    /// 到达站中文名、拼音或车站代码
    #[arg(long)]
    to: String,

    /// 乘车日期，格式 YYYY-MM-DD
    #[arg(long)]
    date: NaiveDate,
}

#[derive(Debug, Args)]
struct BuyArgs {
    /// 出发站中文名、拼音或车站代码
    #[arg(long)]
    from: String,

    /// 到达站中文名、拼音或车站代码
    #[arg(long)]
    to: String,

    /// 乘车日期，格式 YYYY-MM-DD
    #[arg(long)]
    date: NaiveDate,

    /// 必须精确匹配的车次，例如 G123
    #[arg(long)]
    train: String,

    /// 席别代码，例如 second_class、hard_sleeper
    #[arg(long, default_value = "second_class")]
    seat: String,

    /// 高铁/动车座位偏好，例如单人 1A、两人 1A1F
    #[arg(long)]
    choose_seats: Option<String>,

    /// 定时提交时间，例如 14:30 或 YYYY-MM-DD HH:MM:SS
    #[arg(long)]
    at: Option<String>,

    /// 跳过真实订单确认；定时提交时必须使用
    #[arg(long)]
    yes: bool,

    /// 本地乘车人 UUID；多位乘客可重复传入
    #[arg(long = "passenger-id", required = true)]
    passenger_id: Vec<Uuid>,
}

#[derive(Debug, Subcommand)]
enum PassengerCommand {
    /// 手动添加一位本地乘车人
    Add(PassengerAddArgs),
    /// 查看本地 SQLite 中的乘车人
    List,
    /// 从当前 12306 账号同步常用联系人
    #[command(name = "12306-list")]
    List12306,
}

#[derive(Debug, Args)]
struct PassengerAddArgs {
    /// 指定本地 UUID；省略时自动生成
    #[arg(long)]
    id: Option<Uuid>,

    /// 与 12306 常用联系人完全一致的姓名
    #[arg(long)]
    name: String,

    /// 脱敏证件号，例如 3101**********1234
    #[arg(long)]
    id_masked: String,

    /// 乘客类型：adult、child、student、disabled_military
    #[arg(long, default_value = "adult")]
    passenger_type: String,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    /// 创建多日期、多席别抢票任务
    Create(Box<TaskCreateArgs>),
    /// 查看所有任务
    List,
    /// 查看任务配置、状态和订单
    Show {
        /// 任务 UUID
        task_id: String,
    },
    /// 启动任务并在当前终端持续运行
    Start {
        /// 任务 UUID
        task_id: String,
    },
    /// 暂停运行中的任务
    Pause {
        /// 任务 UUID
        task_id: String,
    },
    /// 恢复任务并在当前终端持续运行
    Resume {
        /// 任务 UUID
        task_id: String,
    },
    /// 取消任务
    Cancel {
        /// 任务 UUID
        task_id: String,
    },
    /// 查看任务运行日志
    Logs {
        /// 任务 UUID
        task_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum NotifyCommand {
    /// 查看支持的通知类型和当前配置
    Types,
    /// 配置并启用指定通知类型
    Set {
        /// 通知类型；当前支持 feishu
        notification_type: String,
    },
    /// 发送一条测试通知
    Test {
        /// 通知类型；当前支持 feishu
        notification_type: String,
    },
    /// 启用已配置的通知类型
    Enable {
        /// 通知类型；当前支持 feishu
        notification_type: String,
    },
    /// 禁用通知类型但保留配置
    Disable {
        /// 通知类型；当前支持 feishu
        notification_type: String,
    },
    /// 删除指定通知类型的配置
    Remove {
        /// 通知类型；当前支持 feishu
        notification_type: String,
    },
}

#[derive(Debug, Args)]
struct TaskCreateArgs {
    /// 出发站名称
    #[arg(long)]
    from_name: String,

    /// 出发站代码，例如 SHH
    #[arg(long)]
    from_code: String,

    /// 到达站名称
    #[arg(long)]
    to_name: String,

    /// 到达站代码，例如 JXH
    #[arg(long)]
    to_code: String,

    /// 乘车日期；多日期可重复传入
    #[arg(long, required = true)]
    date: Vec<NaiveDate>,

    /// 席别偏好；多个席别可重复传入并按顺序匹配
    #[arg(long = "seat")]
    seat: Vec<String>,

    /// 本地乘车人 UUID；多位乘客可重复传入
    #[arg(long = "passenger-id")]
    passenger_id: Vec<Uuid>,

    /// 没有目标席别时接受无座
    #[arg(long)]
    accept_no_seat: bool,

    /// 只匹配指定车次；可重复传入
    #[arg(long = "include-train")]
    include_train: Vec<String>,

    /// 排除指定车次；可重复传入
    #[arg(long = "exclude-train")]
    exclude_train: Vec<String>,

    /// 允许候补
    #[arg(long)]
    enable_waitlist: bool,

    /// 无符合余票时优先提交候补；必须同时开启候补
    #[arg(long)]
    enable_strong_waitlist: bool,

    /// 新增车次策略：off、notify_only、auto_order
    #[arg(long, default_value = "off")]
    new_train_policy: String,

    /// 独立新增车次任务；只处理首次基线之后出现的车次
    #[arg(long)]
    new_trains_only: bool,

    /// 余票查询间隔毫秒数，最小 1000
    #[arg(long, default_value_t = DEFAULT_QUERY_INTERVAL_MS)]
    query_interval_ms: u64,

    /// 任务备注
    #[arg(long)]
    remark: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rs12306=info,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve(args) => {
            serve(ServerConfig {
                host: args.host,
                port: args.port,
                database_path: cli.database,
            })
            .await?;
        }
        Command::Login(args) => {
            let database = Database::open(&cli.database)?;
            if args.qr {
                let result = login_12306_qr(
                    &args.qr_path,
                    Duration::from_secs(args.qr_timeout_seconds),
                    |path, image| {
                        println!("qr image: {}", path.display());
                        print_terminal_qr(image);
                        println!("scan it with the 12306 app and confirm login.");
                    },
                )
                .await?;
                database.set_session("logged_in", result.cookies.as_deref())?;
                println!("session: logged_in");
                if let Some(username) = result.username {
                    println!("username: {username}");
                }
                return Ok(());
            }

            let username = args.username.unwrap_or_default();
            if username.trim().is_empty() {
                bail!("--username is required");
            }
            if args.verified {
                database.set_session_state("logged_in")?;
                println!("session: logged_in");
            } else if let Some(id_last4) = args.id_last4 {
                if id_last4.trim().len() != 4 {
                    bail!("--id-last4 must be the last 4 characters of the bound ID card");
                }
                let result = request_12306_sms_code(SmsCodeRequest { username, id_last4 }).await?;
                database.set_session("sms_required", result.cookies.as_deref())?;
                println!("session: sms_required");
                println!("message: {}", result.message);
                println!(
                    "next: 12306-rs login --username <name> --sms-code <code> (password will be prompted)"
                );
            } else if let Some(sms_code) = args.sms_code {
                let password = password_or_prompt(args.password)?;
                let result = login_12306_sms(SmsLoginRequest {
                    username,
                    password,
                    sms_code,
                    cookies: database.session_cookies()?,
                })
                .await?;
                match result.result {
                    LoginResult::LoggedIn => {
                        database.set_session("logged_in", result.cookies.as_deref())?;
                        println!("session: logged_in");
                        if let Some(username) = result.username {
                            println!("username: {username}");
                        }
                    }
                    LoginResult::Failed { reason } => {
                        database.set_session_state("failed")?;
                        if reason.contains("核验方式不正确") {
                            bail!(
                                "login failed: {reason} 请先用 `12306-rs login --username <账号> --id-last4 <证件后4位>` 重新获取验证码"
                            );
                        }
                        bail!("login failed: {reason}");
                    }
                    LoginResult::VerificationRequired { .. } => {
                        database.set_session("verification_required", result.cookies.as_deref())?;
                        println!("session: verification_required");
                    }
                }
            } else {
                let password = password_or_prompt(args.password)?;
                let result = login_12306(LoginRequest { username, password }).await?;
                let state = match result.result {
                    LoginResult::LoggedIn => "logged_in",
                    LoginResult::VerificationRequired { .. } => "verification_required",
                    LoginResult::Failed { reason } => {
                        database.set_session_state("failed")?;
                        bail!("login failed: {reason}");
                    }
                };
                database.set_session(state, result.cookies.as_deref())?;
                println!("session: {state}");
                if state == "verification_required" {
                    println!(
                        "open https://kyfw.12306.cn/otn/resources/login.html to complete manual verification."
                    );
                }
            }
        }
        Command::Logout => {
            let database = Database::open(&cli.database)?;
            database.set_session_state("logged_out")?;
            println!("session: logged_out");
        }
        Command::Status => {
            let database = Database::open(&cli.database)?;
            println!("database: {}", cli.database.display());
            println!("tasks: {}", database.task_count()?);
            let mut session = database.session_state()?;
            if session == "logged_in"
                && let Some(cookies) = database.session_cookies()?
            {
                match list_12306_passengers(&cookies).await {
                    Ok(_) => {}
                    Err(RailwayClientError::SessionExpired) => {
                        session = "expired".to_string();
                        database.set_session_state("expired")?;
                    }
                    Err(error) => println!("session_check: unavailable ({error})"),
                }
            }
            println!("session: {session}");
        }
        Command::Query(args) => {
            let tickets = query_12306_tickets(&args.from, &args.to, args.date)
                .await
                .map_err(anyhow::Error::msg)?;
            println!(
                "{:<6} {:<16} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8}",
                "车次",
                "区间",
                "时间",
                "历时",
                "商务",
                "一等",
                "二等",
                "软卧",
                "硬卧",
                "无座",
                "候补"
            );
            for ticket in tickets {
                println!(
                    "{:<6} {:<16} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8} {:<8}",
                    ticket.train_no,
                    format!("{}-{}", ticket.from_name, ticket.to_name),
                    format!("{}-{}", ticket.depart_time, ticket.arrive_time),
                    ticket.duration,
                    ticket.business,
                    ticket.first_class,
                    ticket.second_class,
                    ticket.soft_sleeper,
                    ticket.hard_sleeper,
                    ticket.no_seat,
                    if ticket.waitlist_available {
                        "可候补"
                    } else {
                        "不可候补"
                    }
                );
            }
        }
        Command::Buy(args) => handle_buy_command(cli.database, args).await?,
        Command::Passenger { command } => handle_passenger_command(cli.database, command).await?,
        Command::Task { command } => handle_task_command(cli.database, command).await?,
        Command::Notify { command } => handle_notify_command(cli.database, command).await?,
    }

    Ok(())
}

async fn handle_notify_command(
    database_path: PathBuf,
    command: NotifyCommand,
) -> anyhow::Result<()> {
    let database = Database::open(database_path)?;
    match command {
        NotifyCommand::Types => {
            println!("{:<12} {:<12} {:<12} 配置信息", "类型", "已配置", "已启用");
            for status in notification::notification_types(&database)? {
                println!(
                    "{:<12} {:<12} {:<12} {}",
                    status.notification_type,
                    yes_no(status.configured),
                    yes_no(status.enabled),
                    status.configuration
                );
            }
        }
        NotifyCommand::Set { notification_type } => {
            ensure_feishu_type(&notification_type)?;
            let webhook = rpassword::prompt_password("Feishu webhook: ")?;
            notification::configure_feishu(&database, &webhook)?;
            println!("notification: feishu");
            println!("configured: yes");
            println!("enabled: yes");
        }
        NotifyCommand::Test { notification_type } => {
            ensure_feishu_type(&notification_type)?;
            let message = format!(
                "[12306-rs] 飞书通知测试\n时间: {}\n状态: 通知配置可用",
                Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            notification::test_feishu(&database, &message).await?;
            println!("notification test sent: feishu");
        }
        NotifyCommand::Enable { notification_type } => {
            ensure_feishu_type(&notification_type)?;
            notification::enable_feishu(&database, true)?;
            println!("notification enabled: feishu");
        }
        NotifyCommand::Disable { notification_type } => {
            ensure_feishu_type(&notification_type)?;
            notification::enable_feishu(&database, false)?;
            println!("notification disabled: feishu");
        }
        NotifyCommand::Remove { notification_type } => {
            ensure_feishu_type(&notification_type)?;
            notification::remove_feishu(&database)?;
            println!("notification removed: feishu");
        }
    }
    Ok(())
}

fn ensure_feishu_type(value: &str) -> anyhow::Result<()> {
    if value.eq_ignore_ascii_case("feishu") {
        Ok(())
    } else {
        bail!("unsupported notification type `{value}`; run `12306-rs notify types`")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

async fn handle_buy_command(database_path: PathBuf, args: BuyArgs) -> anyhow::Result<()> {
    let database = Database::open(database_path)?;
    if database.session_state()? != "logged_in" {
        bail!("login required; run `12306-rs login --qr` first");
    }
    let cookies = database
        .session_cookies()?
        .context("login cookies missing; run `12306-rs login --qr` again")?;

    let seat = parse_seat(&args.seat)?;
    let passenger_ids: Vec<_> = args.passenger_id.into_iter().map(PassengerId).collect();
    let passenger_count = passenger_ids.len();
    let (passenger_names, passenger_id_masks) = selected_passengers(&database, &passenger_ids)?;
    let choose_seats = args
        .choose_seats
        .as_deref()
        .map(|value| normalize_choose_seats(value, seat, passenger_ids.len()))
        .transpose()?;

    if let Some(at) = args.at.as_deref() {
        if !args.yes {
            bail!("scheduled real orders require --yes to confirm automatic submission");
        }
        println!("prewarming query before scheduled submission...");
        let _ = query_12306_tickets(&args.from, &args.to, args.date).await;
        sleep_until(at).await?;
    }

    let tickets = query_12306_tickets(&args.from, &args.to, args.date)
        .await
        .map_err(anyhow::Error::msg)?;
    let ticket = tickets
        .into_iter()
        .find(|ticket| {
            ticket.train_no.eq_ignore_ascii_case(&args.train)
                && ticket.can_web_buy
                && !ticket.secret_str.is_empty()
                && seat_inventory(ticket, seat)
                    .is_some_and(|value| inventory_available(value, passenger_ids.len()))
        })
        .with_context(|| {
            format!(
                "train {} is not currently purchasable with {} inventory",
                args.train,
                seat.as_str()
            )
        })?;

    if !args.yes && !confirm_real_order(&ticket, seat, &passenger_names)? {
        println!("cancelled");
        return Ok(());
    }

    let task = NewTicketTask {
        from: Station {
            name: ticket.from_name.clone(),
            code: ticket.from_code.clone(),
        },
        to: Station {
            name: ticket.to_name.clone(),
            code: ticket.to_code.clone(),
        },
        dates: vec![args.date],
        passengers: passenger_ids,
        seat_preferences: vec![seat],
        accept_no_seat: seat == SeatType::NoSeat,
        train_filters: vec![TrainFilter {
            kind: TrainFilterKind::Include,
            train_no: ticket.train_no.clone(),
        }],
        enable_waitlist: false,
        enable_strong_waitlist: false,
        new_train_policy: NewTrainPolicy::Off,
        new_trains_only: false,
        query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
        remark: Some("created by cli buy".to_string()),
    }
    .build()
    .context("invalid buy request")?;

    let task_id = task.id.0.to_string();
    database.save_task(&task)?;
    database.update_task_status(&task_id, TaskStatus::Running)?;
    database.update_task_status(&task_id, TaskStatus::Querying)?;
    database.update_task_status(&task_id, TaskStatus::Submitting)?;
    let queue_database = database.clone();
    let queue_task_id = task_id.clone();
    let order = submit_12306_order_with_queue_updates(
        RealSubmitOrderRequest {
            cookies,
            secret_str: ticket.secret_str.clone(),
            train_date: args.date,
            back_train_date: args.date,
            from_station_name: ticket.from_name.clone(),
            to_station_name: ticket.to_name.clone(),
            seat_type: seat,
            passenger_names,
            passenger_id_masks,
            choose_seats,
        },
        move |update| {
            print_queue_update(&update);
            let _ = queue_database.append_task_log(
                &queue_task_id,
                "info",
                "order_queue_update",
                &queue_update_message(&update),
                None,
            );
        },
    )
    .await;
    let order = match order {
        Ok(order) => order,
        Err(error) => {
            if let Err(record_error) = record_submit_failure(&database, &task_id, &error) {
                eprintln!("failed to record submit error: {record_error}");
            }
            let message = submit_failure_message(
                "购票提交失败",
                &ticket.from_name,
                &ticket.to_name,
                &ticket.train_no,
                args.date,
                seat,
                &error,
            );
            send_task_notification(&database, &task_id, &message).await;
            return Err(error.into());
        }
    };
    let message = order_success_message(
        &ticket.from_name,
        &ticket.to_name,
        &ticket.train_no,
        args.date,
        seat,
        passenger_count,
        &order.order_no,
    );
    send_task_notification(&database, &task_id, &message).await;
    database.save_order(
        &task_id,
        &order.order_no,
        &ticket.train_no,
        &args.date.to_string(),
        seat.as_str(),
    )?;
    let task = database.update_task_status(&task_id, TaskStatus::PendingPayment)?;
    database.append_task_log(
        &task_id,
        "info",
        "payment_required",
        &format!(
            "order submitted: {}; please pay manually on official 12306",
            order.order_no
        ),
        None,
    )?;

    println!("task: {}", task.id);
    println!("train: {}", ticket.train_no);
    println!("seat: {}", seat.as_str());
    println!("order: {}", order.order_no);
    println!("status: {}", task.status);
    println!(
        "payment: please open official 12306 and pay manually; automatic payment is not supported."
    );
    Ok(())
}

fn print_queue_update(update: &OrderQueueUpdate) {
    if let Some(order_no) = &update.order_no {
        println!("queue: order created {order_no}");
        return;
    }
    let wait_time = update
        .wait_time
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let wait_count = update
        .wait_count
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    println!("queue: wait_time={wait_time}s wait_count={wait_count}");
}

fn queue_update_message(update: &OrderQueueUpdate) -> String {
    if let Some(order_no) = &update.order_no {
        return format!("order created: {order_no}");
    }
    format!(
        "queue wait_time={}s wait_count={}",
        update
            .wait_time
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        update
            .wait_count
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    )
}

async fn send_task_notification(database: &Database, task_id: &str, message: &str) -> bool {
    let deliveries = notification::send_enabled(database, message).await;
    let mut delivered = !deliveries.is_empty();
    for delivery in deliveries {
        if let Some(error) = delivery.error {
            delivered = false;
            eprintln!(
                "notification failed [{}]: {error}",
                delivery.notification_type
            );
            let _ = database.append_task_log(
                task_id,
                "warn",
                "notification_failed",
                &format!("{}: {error}", delivery.notification_type),
                None,
            );
        } else {
            println!("notification sent: {}", delivery.notification_type);
            let _ = database.append_task_log(
                task_id,
                "info",
                "notification_sent",
                delivery.notification_type,
                None,
            );
        }
    }
    delivered
}

fn order_success_message(
    from: &str,
    to: &str,
    train: &str,
    date: NaiveDate,
    seat: SeatType,
    passenger_count: usize,
    order_no: &str,
) -> String {
    format!(
        "[12306-rs] 购票成功，等待支付\n线路: {from} -> {to}\n车次: {train}\n日期: {date}\n席别: {}\n乘客: {passenger_count} 人\n订单号: {}\n请尽快前往官方 12306 完成支付。",
        seat.as_str(),
        masked_reference(order_no)
    )
}

fn waitlist_success_message(
    from: &str,
    to: &str,
    train: &str,
    date: NaiveDate,
    seat: SeatType,
    passenger_count: usize,
    standby_no: Option<&str>,
) -> String {
    let standby_no = standby_no
        .map(masked_reference)
        .unwrap_or_else(|| "-".to_string());
    format!(
        "[12306-rs] 候补已提交\n线路: {from} -> {to}\n车次: {train}\n日期: {date}\n席别: {}\n乘客: {passenger_count} 人\n候补单号: {standby_no}\n请前往官方 12306 检查确认或支付状态。",
        seat.as_str()
    )
}

fn submit_failure_message(
    title: &str,
    from: &str,
    to: &str,
    train: &str,
    date: NaiveDate,
    seat: SeatType,
    error: &RailwayClientError,
) -> String {
    format!(
        "[12306-rs] {title}\n线路: {from} -> {to}\n车次: {train}\n日期: {date}\n席别: {}\n12306: {}\n请检查后重试。",
        seat.as_str(),
        notification::truncate(&error.to_string(), 800)
    )
}

fn masked_reference(value: &str) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= 4 {
        return "****".to_string();
    }
    format!(
        "****{}",
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

fn selected_passengers(
    database: &Database,
    passenger_ids: &[PassengerId],
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let passengers = database.list_passengers()?;
    passenger_ids
        .iter()
        .map(|id| {
            passengers
                .iter()
                .find(|passenger| passenger.id == *id)
                .map(|passenger| (passenger.name.clone(), passenger.id_masked.clone()))
                .with_context(|| format!("passenger not found locally: {}", id.0))
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .map(|passengers| passengers.into_iter().unzip())
}

fn confirm_real_order(
    ticket: &rs12306_server::TicketQueryRow,
    seat: SeatType,
    passenger_names: &[String],
) -> anyhow::Result<bool> {
    println!(
        "order: {} {}->{} {} {} passengers={}",
        ticket.train_no,
        ticket.from_name,
        ticket.to_name,
        ticket.date,
        seat.as_str(),
        passenger_names.join(",")
    );
    print!("submit this real order? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn record_submit_failure(
    database: &Database,
    task_id: &str,
    error: &RailwayClientError,
) -> anyhow::Result<()> {
    let next_status = match error {
        RailwayClientError::SessionExpired => {
            database.set_session_state("expired")?;
            TaskStatus::WaitingLogin
        }
        RailwayClientError::VerificationRequired => TaskStatus::VerificationRequired,
        _ => TaskStatus::Failed,
    };
    database.update_task_status(task_id, next_status)?;
    database.append_task_log(
        task_id,
        "error",
        "order_submit_failed",
        &error.to_string(),
        None,
    )?;
    Ok(())
}

async fn handle_passenger_command(
    database_path: PathBuf,
    command: PassengerCommand,
) -> anyhow::Result<()> {
    let database = Database::open(database_path)?;

    match command {
        PassengerCommand::Add(args) => {
            let name = args.name.trim();
            let id_masked = args.id_masked.trim();
            if name.is_empty() {
                bail!("--name is required");
            }
            if id_masked.is_empty() {
                bail!("--id-masked is required");
            }
            let passenger = Passenger {
                id: PassengerId(args.id.unwrap_or_else(Uuid::new_v4)),
                name: name.to_string(),
                id_masked: id_masked.to_string(),
                passenger_type: parse_passenger_type(&args.passenger_type)?,
            };
            database.save_passenger(&passenger)?;
            println!("passenger: {}", passenger.id.0);
            println!("name: {}", passenger.name);
            println!("type: {}", passenger.passenger_type.as_str());
        }
        PassengerCommand::List => {
            let passengers = database.list_passengers()?;
            if passengers.is_empty() {
                println!("no passengers");
            } else {
                for passenger in passengers {
                    println!(
                        "{} {} {} {}",
                        passenger.id.0,
                        passenger.name,
                        passenger.id_masked,
                        passenger.passenger_type.as_str()
                    );
                }
            }
        }
        PassengerCommand::List12306 => {
            if database.session_state()? != "logged_in" {
                bail!("login required; run `12306-rs login --qr` first");
            }
            let cookies = database
                .session_cookies()?
                .context("login cookies missing; run `12306-rs login --qr` again")?;
            let passengers = match list_12306_passengers(&cookies).await {
                Ok(passengers) => passengers,
                Err(RailwayClientError::SessionExpired) => {
                    bail!("session expired; run `12306-rs login --qr` again")
                }
                Err(error) => return Err(error.into()),
            };
            if passengers.is_empty() {
                println!("no passengers");
            } else {
                let mut local_passengers = database.list_passengers()?;
                println!(
                    "{:<36} {:<12} {:<8} {:<24} {:<16}",
                    "本地ID", "姓名", "票种", "证件号", "手机号"
                );
                for passenger in passengers {
                    let id_masked = mask_middle(&passenger.id_no, 4, 4);
                    let local = local_passengers
                        .iter()
                        .find(|local| local.name == passenger.name && local.id_masked == id_masked)
                        .cloned()
                        .unwrap_or_else(|| Passenger {
                            id: PassengerId::new(),
                            name: passenger.name.clone(),
                            id_masked: id_masked.clone(),
                            passenger_type: railway_passenger_type(&passenger.passenger_type),
                        });
                    database.save_passenger(&local)?;
                    if !local_passengers.iter().any(|saved| saved.id == local.id) {
                        local_passengers.push(local.clone());
                    }
                    println!(
                        "{:<36} {:<12} {:<8} {:<24} {:<16}",
                        local.id.0,
                        passenger.name,
                        passenger.passenger_type,
                        id_masked,
                        mask_middle(&passenger.mobile_no, 3, 4)
                    );
                }
            }
        }
    }

    Ok(())
}

async fn handle_task_command(database_path: PathBuf, command: TaskCommand) -> anyhow::Result<()> {
    let database = Database::open(database_path)?;

    match command {
        TaskCommand::Create(args) => {
            let new_train_policy = parse_new_train_policy(&args.new_train_policy)?;
            if args.passenger_id.is_empty()
                && !(args.new_trains_only && new_train_policy == NewTrainPolicy::NotifyOnly)
            {
                bail!("at least one --passenger-id is required");
            }
            let passenger_ids: Vec<_> = args.passenger_id.into_iter().map(PassengerId).collect();
            selected_passengers(&database, &passenger_ids)?;
            let task = NewTicketTask {
                from: Station {
                    name: args.from_name,
                    code: args.from_code,
                },
                to: Station {
                    name: args.to_name,
                    code: args.to_code,
                },
                dates: args.date,
                passengers: passenger_ids,
                seat_preferences: parse_seats(&args.seat)?,
                accept_no_seat: args.accept_no_seat,
                train_filters: parse_train_filters(args.include_train, args.exclude_train),
                enable_waitlist: args.enable_waitlist,
                enable_strong_waitlist: args.enable_strong_waitlist,
                new_train_policy,
                new_trains_only: args.new_trains_only,
                query_interval_ms: Some(args.query_interval_ms),
                remark: args.remark,
            }
            .build()
            .context("invalid task configuration")?;

            database.save_task(&task)?;
            println!("created task: {}", task.id.0);
            println!("status: {}", task.status.as_str());
            println!("query_interval_ms: {}", task.query_interval_ms);
        }
        TaskCommand::List => {
            let tasks = database.list_task_summaries()?;
            if tasks.is_empty() {
                println!("no tasks");
            } else {
                for task in tasks {
                    println!(
                        "{} {}->{} status={} interval={}ms updated_at={}",
                        task.id,
                        task.from_name,
                        task.to_name,
                        task.status,
                        task.query_interval_ms,
                        task.updated_at
                    );
                }
            }
        }
        TaskCommand::Show { task_id } => {
            let task = database.get_task_details(&task_id)?;
            println!("id: {}", task.id);
            println!(
                "route: {}({}) -> {}({})",
                task.from_name, task.from_code, task.to_name, task.to_code
            );
            println!("dates: {}", task.dates.join(", "));
            println!("passengers: {}", task.passenger_ids.join(", "));
            println!("seats: {}", task.seat_types.join(", "));
            println!("include trains: {}", task.train_include.join(", "));
            println!("exclude trains: {}", task.train_exclude.join(", "));
            println!("accept_no_seat: {}", task.accept_no_seat);
            println!("enable_waitlist: {}", task.enable_waitlist);
            println!("enable_strong_waitlist: {}", task.enable_strong_waitlist);
            println!("new_train_policy: {}", task.new_train_policy);
            println!("new_trains_only: {}", task.new_trains_only);
            println!("query_interval_ms: {}", task.query_interval_ms);
            println!("status: {}", task.status);
            println!("remark: {}", task.remark.unwrap_or_default());
            println!("created_at: {}", task.created_at);
            println!("updated_at: {}", task.updated_at);
            let new_trains = database.list_new_trains(&task_id)?;
            if new_trains.is_empty() {
                println!("new trains: none");
            } else {
                println!("new trains:");
                for train in new_trains {
                    println!(
                        "  {} {} added_notified={} available_notified={} first_seen_at={}",
                        train.travel_date,
                        train.train_no,
                        train.added_notified,
                        train.available_notified,
                        train.first_seen_at
                    );
                }
            }
            if let Some(order) = database.order_for_task(&task_id)? {
                println!("order: {}", order.order_no);
                println!(
                    "order_detail: {} {} {} {}",
                    order.train_no, order.travel_date, order.seat_type, order.state
                );
            }
        }
        TaskCommand::Start { task_id } => {
            update_task_status(&database, &task_id, TaskStatus::Running)?;
            run_task(database.clone(), task_id).await?;
        }
        TaskCommand::Pause { task_id } => {
            update_task_status(&database, &task_id, TaskStatus::Paused)?
        }
        TaskCommand::Resume { task_id } => {
            update_task_status(&database, &task_id, TaskStatus::Running)?;
            run_task(database.clone(), task_id).await?;
        }
        TaskCommand::Cancel { task_id } => {
            update_task_status(&database, &task_id, TaskStatus::Cancelled)?
        }
        TaskCommand::Logs { task_id } => {
            let logs = database.list_task_logs(&task_id)?;
            if logs.is_empty() {
                println!("no logs");
            } else {
                for log in logs {
                    println!(
                        "{} [{}] {} {}",
                        log.created_at, log.level, log.event, log.message
                    );
                }
            }
        }
    }

    Ok(())
}

async fn run_task(database: Database, task_id: String) -> anyhow::Result<()> {
    loop {
        let task = database.get_task_details(&task_id)?;
        match task.status.as_str() {
            "running" => {
                database.update_task_status(&task_id, TaskStatus::Querying)?;
            }
            "querying" => {}
            "paused" | "cancelled" | "failed" | "pending_payment" => {
                println!("task: {task_id}");
                println!("status: {}", task.status);
                return Ok(());
            }
            status => bail!("task cannot run from status {status}"),
        }

        let new_train_policy = parse_new_train_policy(&task.new_train_policy)?;
        let monitor_only = task.new_trains_only && new_train_policy == NewTrainPolicy::NotifyOnly;
        let cookies = if monitor_only {
            None
        } else {
            if database.session_state()? != "logged_in" {
                database.update_task_status(&task_id, TaskStatus::WaitingLogin)?;
                bail!("login required; run `12306-rs login --qr` first");
            }
            Some(
                database
                    .session_cookies()?
                    .context("login cookies missing; run `12306-rs login --qr` again")?,
            )
        };
        let passenger_ids = task
            .passenger_ids
            .iter()
            .map(|id| Uuid::parse_str(id).map(PassengerId))
            .collect::<Result<Vec<_>, _>>()?;
        let (passenger_names, passenger_id_masks) = selected_passengers(&database, &passenger_ids)?;
        let mut seats = parse_seats(&task.seat_types)?;
        if task.accept_no_seat && !seats.contains(&SeatType::NoSeat) {
            seats.push(SeatType::NoSeat);
        }

        let snapshots = match query_task_snapshots(&database, &task, &seats).await {
            Ok(snapshots) => snapshots,
            Err(error) => {
                database.append_task_log(&task_id, "warn", "ticket_query_failed", &error, None)?;
                println!(
                    "query failed: {error}; retrying in {}ms",
                    task.query_interval_ms
                );
                tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
                continue;
            }
        };

        if monitor_only {
            println!(
                "task: {task_id} monitoring new trains; next query in {}ms",
                task.query_interval_ms
            );
            tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
            continue;
        }
        let cookies = cookies.expect("purchasing tasks require login cookies");

        let selection = select_task_ticket(&task, &seats, &snapshots);
        let (ticket, date, seat) = match selection {
            Some(selection) => selection,
            None if task.enable_waitlist && task.enable_strong_waitlist => {
                match select_waitlist_ticket(&task, &seats, &snapshots) {
                    Some((ticket, date, seat)) => {
                        if !database.try_claim_train_action(
                            &task_id,
                            &date.to_string(),
                            &ticket.train_no,
                            "waitlist",
                        )? {
                            database.append_task_log(
                                &task_id,
                                "warn",
                                "duplicate_waitlist_skipped",
                                "duplicate waitlist submission skipped",
                                None,
                            )?;
                            tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
                            continue;
                        }
                        database.update_task_status(&task_id, TaskStatus::CandidateSubmitting)?;
                        let waitlist = submit_12306_waitlist(RealSubmitWaitlistRequest {
                            cookies: cookies.clone(),
                            secret_str: ticket.secret_str.clone(),
                            train_id: ticket.train_id.clone(),
                            seat_type: seat,
                            passenger_names,
                            passenger_id_masks,
                        })
                        .await;
                        let waitlist = match waitlist {
                            Ok(waitlist) => waitlist,
                            Err(error) => {
                                if let Err(record_error) =
                                    record_waitlist_failure(&database, &task_id, &error)
                                {
                                    eprintln!("failed to record waitlist error: {record_error}");
                                }
                                let message = submit_failure_message(
                                    "候补提交失败",
                                    &ticket.from_name,
                                    &ticket.to_name,
                                    &ticket.train_no,
                                    date,
                                    seat,
                                    &error,
                                );
                                send_task_notification(&database, &task_id, &message).await;
                                return Err(error.into());
                            }
                        };
                        let message = waitlist_success_message(
                            &ticket.from_name,
                            &ticket.to_name,
                            &ticket.train_no,
                            date,
                            seat,
                            passenger_ids.len(),
                            waitlist.standby_no.as_deref(),
                        );
                        send_task_notification(&database, &task_id, &message).await;
                        database.save_standby_order(&task_id, waitlist.standby_no.as_deref())?;
                        database.update_task_status(&task_id, TaskStatus::CandidateSubmitted)?;
                        database.append_task_log(
                            &task_id,
                            "info",
                            "waitlist_submitted",
                            "waitlist submitted; check official 12306 for confirmation or payment",
                            None,
                        )?;
                        println!("task: {task_id}");
                        if let Some(standby_no) = waitlist.standby_no {
                            println!("standby_order: {standby_no}");
                        }
                        println!("status: candidate_submitted");
                        println!("next: check official 12306 for waitlist confirmation or payment");
                        return Ok(());
                    }
                    None => {
                        println!(
                            "task: {task_id} no matching ticket or waitlist opportunity; next query in {}ms",
                            task.query_interval_ms
                        );
                        tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
                        continue;
                    }
                }
            }
            None => {
                println!(
                    "task: {task_id} no matching inventory; next query in {}ms",
                    task.query_interval_ms
                );
                tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
                continue;
            }
        };

        if !database.try_claim_train_action(
            &task_id,
            &date.to_string(),
            &ticket.train_no,
            "order",
        )? {
            database.append_task_log(
                &task_id,
                "warn",
                "duplicate_order_skipped",
                "duplicate order submission skipped",
                None,
            )?;
            tokio::time::sleep(Duration::from_millis(task.query_interval_ms)).await;
            continue;
        }
        database.update_task_status(&task_id, TaskStatus::Submitting)?;
        let queue_database = database.clone();
        let queue_task_id = task_id.clone();
        let order = submit_12306_order_with_queue_updates(
            RealSubmitOrderRequest {
                cookies,
                secret_str: ticket.secret_str.clone(),
                train_date: date,
                back_train_date: date,
                from_station_name: ticket.from_name.clone(),
                to_station_name: ticket.to_name.clone(),
                seat_type: seat,
                passenger_names,
                passenger_id_masks,
                choose_seats: None,
            },
            move |update| {
                print_queue_update(&update);
                let _ = queue_database.append_task_log(
                    &queue_task_id,
                    "info",
                    "order_queue_update",
                    &queue_update_message(&update),
                    None,
                );
            },
        )
        .await;
        let order = match order {
            Ok(order) => order,
            Err(error) => {
                if let Err(record_error) = record_submit_failure(&database, &task_id, &error) {
                    eprintln!("failed to record submit error: {record_error}");
                }
                let message = submit_failure_message(
                    "购票提交失败",
                    &ticket.from_name,
                    &ticket.to_name,
                    &ticket.train_no,
                    date,
                    seat,
                    &error,
                );
                send_task_notification(&database, &task_id, &message).await;
                return Err(error.into());
            }
        };
        let message = order_success_message(
            &ticket.from_name,
            &ticket.to_name,
            &ticket.train_no,
            date,
            seat,
            passenger_ids.len(),
            &order.order_no,
        );
        send_task_notification(&database, &task_id, &message).await;
        database.save_order(
            &task_id,
            &order.order_no,
            &ticket.train_no,
            &date.to_string(),
            seat.as_str(),
        )?;
        database.update_task_status(&task_id, TaskStatus::PendingPayment)?;
        database.append_task_log(
            &task_id,
            "info",
            "payment_required",
            &format!(
                "order submitted: {}; please pay manually on official 12306",
                order.order_no
            ),
            None,
        )?;
        println!("task: {task_id}");
        println!("order: {}", order.order_no);
        println!("status: pending_payment");
        println!("payment: please pay manually on official 12306");
        return Ok(());
    }
}

#[derive(Debug)]
struct TaskQuerySnapshot {
    date: NaiveDate,
    tickets: Vec<rs12306_server::TicketQueryRow>,
    new_trains: HashSet<String>,
}

async fn query_task_snapshots(
    database: &Database,
    task: &rs12306_storage::TaskDetails,
    seats: &[SeatType],
) -> Result<Vec<TaskQuerySnapshot>, String> {
    let mut last_error = None;
    let mut snapshots = Vec::new();
    let monitoring_enabled = task.new_train_policy != NewTrainPolicy::Off.as_str();
    for date in &task.dates {
        let date =
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|error| error.to_string())?;
        let tickets = match query_12306_tickets(&task.from_code, &task.to_code, date).await {
            Ok(tickets) => tickets,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };

        let mut new_trains = HashSet::new();
        if monitoring_enabled {
            let train_numbers = tickets
                .iter()
                .map(|ticket| ticket.train_no.clone())
                .collect::<Vec<_>>();
            let observed = database
                .observe_task_trains(&task.id, &date.to_string(), &train_numbers)
                .map_err(|error| error.to_string())?;
            if observed.baseline_created {
                database
                    .append_task_log(
                        &task.id,
                        "info",
                        "new_train_baseline_created",
                        &format!(
                            "new-train baseline created for {date}: {} trains",
                            tickets.len()
                        ),
                        None,
                    )
                    .map_err(|error| error.to_string())?;
            }
            for observation in observed
                .observations
                .into_iter()
                .filter(|observation| observation.is_new)
            {
                new_trains.insert(observation.train_no.clone());
                let Some(ticket) = tickets.iter().find(|ticket| {
                    ticket.train_no.eq_ignore_ascii_case(&observation.train_no)
                        && task_ticket_matches(task, ticket)
                }) else {
                    continue;
                };
                if observation.first_observed {
                    database
                        .append_task_log(
                            &task.id,
                            "info",
                            "new_train_discovered",
                            &format!(
                                "new train discovered: {} {} {}-{}",
                                ticket.train_no, date, ticket.depart_time, ticket.arrive_time
                            ),
                            None,
                        )
                        .map_err(|error| error.to_string())?;
                }
                if !observation.added_notified
                    && send_task_notification(
                        database,
                        &task.id,
                        &new_train_added_message(task, ticket, date),
                    )
                    .await
                {
                    database
                        .mark_new_train_added_notified(
                            &task.id,
                            &date.to_string(),
                            &ticket.train_no,
                        )
                        .map_err(|error| error.to_string())?;
                }
                if !observation.available_notified
                    && let Some(seat) = matching_available_seat(task, seats, ticket)
                    && send_task_notification(
                        database,
                        &task.id,
                        &new_train_available_message(task, ticket, date, seat),
                    )
                    .await
                {
                    database
                        .mark_new_train_available_notified(
                            &task.id,
                            &date.to_string(),
                            &ticket.train_no,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        snapshots.push(TaskQuerySnapshot {
            date,
            tickets,
            new_trains,
        });
    }
    if snapshots.is_empty()
        && let Some(error) = last_error
    {
        Err(error)
    } else {
        Ok(snapshots)
    }
}

fn select_task_ticket(
    task: &rs12306_storage::TaskDetails,
    seats: &[SeatType],
    snapshots: &[TaskQuerySnapshot],
) -> Option<(rs12306_server::TicketQueryRow, NaiveDate, SeatType)> {
    for snapshot in snapshots {
        for seat in seats {
            if let Some(ticket) = snapshot.tickets.iter().find(|ticket| {
                task_ticket_matches(task, ticket)
                    && task_train_action_allowed(task, snapshot, ticket)
                    && ticket.can_web_buy
                    && !ticket.secret_str.is_empty()
                    && seat_inventory(ticket, *seat)
                        .is_some_and(|value| inventory_available(value, task.passenger_ids.len()))
            }) {
                return Some((ticket.clone(), snapshot.date, *seat));
            }
        }
    }
    None
}

fn select_waitlist_ticket(
    task: &rs12306_storage::TaskDetails,
    seats: &[SeatType],
    snapshots: &[TaskQuerySnapshot],
) -> Option<(rs12306_server::TicketQueryRow, NaiveDate, SeatType)> {
    for snapshot in snapshots {
        for seat in seats {
            if let Some(ticket) = snapshot.tickets.iter().find(|ticket| {
                task_ticket_matches(task, ticket)
                    && task_train_action_allowed(task, snapshot, ticket)
                    && ticket.waitlist_available
                    && !ticket.secret_str.is_empty()
                    && !ticket.train_id.is_empty()
                    && !ticket.waitlist_seat_codes.contains(seat_code(*seat))
            }) {
                return Some((ticket.clone(), snapshot.date, *seat));
            }
        }
    }
    None
}

fn task_train_action_allowed(
    task: &rs12306_storage::TaskDetails,
    snapshot: &TaskQuerySnapshot,
    ticket: &rs12306_server::TicketQueryRow,
) -> bool {
    let is_new = snapshot
        .new_trains
        .contains(&ticket.train_no.to_uppercase());
    if task.new_trains_only && !is_new {
        return false;
    }
    !(is_new && task.new_train_policy == NewTrainPolicy::NotifyOnly.as_str())
}

fn matching_available_seat(
    task: &rs12306_storage::TaskDetails,
    seats: &[SeatType],
    ticket: &rs12306_server::TicketQueryRow,
) -> Option<SeatType> {
    seats.iter().copied().find(|seat| {
        ticket.can_web_buy
            && !ticket.secret_str.is_empty()
            && seat_inventory(ticket, *seat)
                .is_some_and(|value| inventory_available(value, task.passenger_ids.len().max(1)))
    })
}

fn new_train_added_message(
    task: &rs12306_storage::TaskDetails,
    ticket: &rs12306_server::TicketQueryRow,
    date: NaiveDate,
) -> String {
    format!(
        "[12306-rs] 发现新增车次\n任务: {}\n线路: {} -> {}\n车次: {}\n日期: {}\n时间: {}-{}\n历时: {}\n余票: {}",
        &task.id[..task.id.len().min(8)],
        ticket.from_name,
        ticket.to_name,
        ticket.train_no,
        date,
        ticket.depart_time,
        ticket.arrive_time,
        ticket.duration,
        train_inventory_summary(ticket)
    )
}

fn new_train_available_message(
    task: &rs12306_storage::TaskDetails,
    ticket: &rs12306_server::TicketQueryRow,
    date: NaiveDate,
    seat: SeatType,
) -> String {
    format!(
        "[12306-rs] 新增车次已有余票\n任务: {}\n线路: {} -> {}\n车次: {}\n日期: {}\n时间: {}-{}\n匹配席别: {}\n余票: {}",
        &task.id[..task.id.len().min(8)],
        ticket.from_name,
        ticket.to_name,
        ticket.train_no,
        date,
        ticket.depart_time,
        ticket.arrive_time,
        seat.as_str(),
        train_inventory_summary(ticket)
    )
}

fn train_inventory_summary(ticket: &rs12306_server::TicketQueryRow) -> String {
    format!(
        "商务 {} / 一等 {} / 二等 {} / 软卧 {} / 硬卧 {} / 硬座 {} / 无座 {}",
        ticket.business,
        ticket.first_class,
        ticket.second_class,
        ticket.soft_sleeper,
        ticket.hard_sleeper,
        ticket.hard_seat,
        ticket.no_seat
    )
}

fn task_ticket_matches(
    task: &rs12306_storage::TaskDetails,
    ticket: &rs12306_server::TicketQueryRow,
) -> bool {
    (task.train_include.is_empty()
        || task
            .train_include
            .iter()
            .any(|train| ticket.train_no.eq_ignore_ascii_case(train)))
        && !task
            .train_exclude
            .iter()
            .any(|train| ticket.train_no.eq_ignore_ascii_case(train))
}

fn record_waitlist_failure(
    database: &Database,
    task_id: &str,
    error: &RailwayClientError,
) -> anyhow::Result<()> {
    let next_status = match error {
        RailwayClientError::SessionExpired => {
            database.set_session_state("expired")?;
            TaskStatus::WaitingLogin
        }
        RailwayClientError::VerificationRequired => TaskStatus::VerificationRequired,
        _ => TaskStatus::Failed,
    };
    database.update_task_status(task_id, next_status)?;
    database.append_task_log(
        task_id,
        "error",
        "waitlist_submit_failed",
        &error.to_string(),
        None,
    )?;
    Ok(())
}

fn update_task_status(
    database: &Database,
    task_id: &str,
    status: TaskStatus,
) -> anyhow::Result<()> {
    let task = database.update_task_status(task_id, status)?;
    println!("task: {}", task.id);
    println!("status: {}", task.status);
    println!("updated_at: {}", task.updated_at);
    Ok(())
}

fn seat_inventory(ticket: &rs12306_server::TicketQueryRow, seat: SeatType) -> Option<&str> {
    match seat {
        SeatType::Business => Some(&ticket.business),
        SeatType::FirstClass => Some(&ticket.first_class),
        SeatType::SecondClass => Some(&ticket.second_class),
        SeatType::SoftSleeper => Some(&ticket.soft_sleeper),
        SeatType::HardSleeper => Some(&ticket.hard_sleeper),
        SeatType::HardSeat => Some(&ticket.hard_seat),
        SeatType::NoSeat => Some(&ticket.no_seat),
    }
    .map(String::as_str)
}

fn inventory_available(value: &str, passenger_count: usize) -> bool {
    matches!(value.trim(), "有" | "充足")
        || value
            .trim()
            .parse::<usize>()
            .is_ok_and(|count| count >= passenger_count)
}

async fn sleep_until(value: &str) -> anyhow::Result<()> {
    let target = parse_schedule_at(value)?;
    let now = Local::now();
    if target > now {
        let wait = target
            .signed_duration_since(now)
            .to_std()
            .context("invalid scheduled time")?;
        println!("scheduled_at: {}", target.format("%Y-%m-%d %H:%M:%S"));
        println!("waiting_seconds: {}", wait.as_secs());
        tokio::time::sleep(wait).await;
    }
    Ok(())
}

fn parse_schedule_at(value: &str) -> anyhow::Result<DateTime<Local>> {
    let value = value.trim();
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
    {
        return local_datetime(naive);
    }

    let time = NaiveTime::parse_from_str(value, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
        .with_context(
            || "--at must be `YYYY-MM-DD HH:MM:SS`, `YYYY-MM-DDTHH:MM:SS`, `HH:MM:SS`, or `HH:MM`",
        )?;
    let now = Local::now();
    let mut target = local_datetime(NaiveDateTime::new(now.date_naive(), time))?;
    if target <= now {
        target = local_datetime(NaiveDateTime::new(
            now.date_naive()
                .succ_opt()
                .context("failed to calculate tomorrow")?,
            time,
        ))?;
    }
    Ok(target)
}

fn local_datetime(value: NaiveDateTime) -> anyhow::Result<DateTime<Local>> {
    Local
        .from_local_datetime(&value)
        .single()
        .context("scheduled time is ambiguous or invalid in local timezone")
}

fn normalize_choose_seats(
    value: &str,
    seat: SeatType,
    passenger_count: usize,
) -> anyhow::Result<String> {
    if !matches!(
        seat,
        SeatType::Business | SeatType::FirstClass | SeatType::SecondClass
    ) {
        bail!("--choose-seats is only supported for business, first_class, and second_class");
    }
    let normalized = value
        .chars()
        .filter(|char| !char.is_whitespace() && *char != ',')
        .collect::<String>()
        .to_uppercase();
    if normalized.is_empty() {
        bail!("--choose-seats cannot be empty");
    }
    let chars: Vec<_> = normalized.chars().collect();
    if chars.len() != passenger_count * 2
        || chars.chunks_exact(2).any(|pair| {
            let valid_letter = match seat {
                SeatType::Business => matches!(pair[1], 'A' | 'C' | 'F'),
                SeatType::FirstClass => matches!(pair[1], 'A' | 'C' | 'D' | 'F'),
                SeatType::SecondClass => matches!(pair[1], 'A' | 'B' | 'C' | 'D' | 'F'),
                _ => false,
            };
            !pair[0].is_ascii_digit() || !valid_letter
        })
    {
        bail!("--choose-seats requires one digit+letter pair per passenger, e.g. 1A or 1A1F");
    }
    Ok(normalized)
}

fn password_or_prompt(password: Option<String>) -> anyhow::Result<String> {
    let password = match password {
        Some(password) => password,
        None => rpassword::prompt_password("12306 password: ")?,
    };
    if password.trim().is_empty() {
        bail!("password is required; use the hidden prompt or RS12306_PASSWORD");
    }
    Ok(password)
}

fn railway_passenger_type(value: &str) -> PassengerType {
    match value {
        "2" => PassengerType::Child,
        "3" => PassengerType::Student,
        "4" => PassengerType::DisabledMilitary,
        _ => PassengerType::Adult,
    }
}

fn parse_train_filters(include: Vec<String>, exclude: Vec<String>) -> Vec<TrainFilter> {
    let includes = include.into_iter().map(|train_no| TrainFilter {
        kind: TrainFilterKind::Include,
        train_no,
    });
    let excludes = exclude.into_iter().map(|train_no| TrainFilter {
        kind: TrainFilterKind::Exclude,
        train_no,
    });
    includes.chain(excludes).collect()
}

fn parse_new_train_policy(value: &str) -> anyhow::Result<NewTrainPolicy> {
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("--new-train-policy must be off, notify_only, or auto_order"))
}

fn mask_middle(value: &str, keep_start: usize, keep_end: usize) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= keep_start + keep_end {
        return value.to_string();
    }
    format!(
        "{}{}{}",
        chars[..keep_start].iter().collect::<String>(),
        "*".repeat(chars.len() - keep_start - keep_end),
        chars[chars.len() - keep_end..].iter().collect::<String>()
    )
}

fn print_terminal_qr(image: &[u8]) {
    if let Some((pixels, width, height, channels)) = decode_png(image) {
        let cell = (width / 25).max(1);
        println!();
        for y in (0..height).step_by(cell) {
            let mut line = String::new();
            for x in (0..width).step_by(cell) {
                let dark = is_dark_cell(&pixels, width, height, channels, x, y, cell);
                line.push_str(if dark {
                    "\x1b[40m  \x1b[0m"
                } else {
                    "\x1b[47m  \x1b[0m"
                });
            }
            println!("{line}");
        }
        println!();
    }
}

fn decode_png(image: &[u8]) -> Option<(Vec<u8>, usize, usize, usize)> {
    let decoder = png::Decoder::new(Cursor::new(image));
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        _ => return None,
    };
    Some((
        buffer[..info.buffer_size()].to_vec(),
        info.width as usize,
        info.height as usize,
        channels,
    ))
}

fn is_dark_cell(
    pixels: &[u8],
    width: usize,
    height: usize,
    channels: usize,
    x: usize,
    y: usize,
    cell: usize,
) -> bool {
    let px = (x + cell / 2).min(width - 1);
    let py = (y + cell / 2).min(height - 1);
    let offset = (py * width + px) * channels;
    let lightness = if channels == 1 || channels == 2 {
        pixels[offset] as u16
    } else {
        (pixels[offset] as u16 + pixels[offset + 1] as u16 + pixels[offset + 2] as u16) / 3
    };
    lightness < 128
}

fn parse_passenger_type(value: &str) -> anyhow::Result<PassengerType> {
    match value {
        "adult" => Ok(PassengerType::Adult),
        "child" => Ok(PassengerType::Child),
        "student" => Ok(PassengerType::Student),
        "disabled_military" => Ok(PassengerType::DisabledMilitary),
        _ => bail!(
            "unknown passenger type `{value}`; use one of adult, child, student, disabled_military"
        ),
    }
}

fn parse_seats(values: &[String]) -> anyhow::Result<Vec<SeatType>> {
    values.iter().map(|value| parse_seat(value)).collect()
}

fn parse_seat(value: &str) -> anyhow::Result<SeatType> {
    match value {
        "business" => Ok(SeatType::Business),
        "first_class" => Ok(SeatType::FirstClass),
        "second_class" => Ok(SeatType::SecondClass),
        "soft_sleeper" => Ok(SeatType::SoftSleeper),
        "hard_sleeper" => Ok(SeatType::HardSleeper),
        "hard_seat" => Ok(SeatType::HardSeat),
        "no_seat" => Ok(SeatType::NoSeat),
        _ => bail!(
            "unknown seat type `{value}`; use one of business, first_class, second_class, soft_sleeper, hard_sleeper, hard_seat, no_seat"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_details(policy: NewTrainPolicy, new_trains_only: bool) -> rs12306_storage::TaskDetails {
        rs12306_storage::TaskDetails {
            id: Uuid::new_v4().to_string(),
            from_name: "上海".to_string(),
            from_code: "SHH".to_string(),
            to_name: "嘉兴".to_string(),
            to_code: "JXH".to_string(),
            accept_no_seat: false,
            enable_waitlist: false,
            enable_strong_waitlist: false,
            new_train_policy: policy.as_str().to_string(),
            new_trains_only,
            query_interval_ms: DEFAULT_QUERY_INTERVAL_MS,
            status: "querying".to_string(),
            remark: None,
            created_at: "2026-07-01T00:00:00Z".to_string(),
            updated_at: "2026-07-01T00:00:00Z".to_string(),
            dates: vec!["2026-07-10".to_string()],
            passenger_ids: vec![Uuid::new_v4().to_string()],
            seat_types: vec!["second_class".to_string()],
            train_include: Vec::new(),
            train_exclude: Vec::new(),
        }
    }

    fn ticket(train_no: &str) -> rs12306_server::TicketQueryRow {
        rs12306_server::TicketQueryRow {
            secret_str: "secret".to_string(),
            train_id: "train-id".to_string(),
            train_no: train_no.to_string(),
            from_code: "SHH".to_string(),
            from_name: "上海".to_string(),
            to_code: "JXH".to_string(),
            to_name: "嘉兴".to_string(),
            date: "2026-07-10".to_string(),
            depart_time: "08:00".to_string(),
            arrive_time: "09:00".to_string(),
            duration: "01:00".to_string(),
            can_web_buy: true,
            business: "无".to_string(),
            first_class: "无".to_string(),
            second_class: "有".to_string(),
            soft_sleeper: "--".to_string(),
            hard_sleeper: "--".to_string(),
            hard_seat: "--".to_string(),
            no_seat: "无".to_string(),
            waitlist_available: false,
            waitlist_seat_codes: String::new(),
        }
    }

    #[test]
    fn normalizes_choose_seats() {
        assert_eq!(
            normalize_choose_seats("1a, 1f", SeatType::SecondClass, 2).unwrap(),
            "1A1F"
        );
        assert!(normalize_choose_seats("1E", SeatType::SecondClass, 1).is_err());
        assert!(normalize_choose_seats("123", SeatType::SecondClass, 1).is_err());
        assert!(normalize_choose_seats("1A", SeatType::HardSleeper, 1).is_err());
        assert!(normalize_choose_seats("1B", SeatType::Business, 1).is_err());
    }

    #[test]
    fn parses_absolute_schedule_time() {
        assert!(parse_schedule_at("2026-07-08 14:30:00").is_ok());
        assert!(parse_schedule_at("bad").is_err());
    }

    #[test]
    fn requires_enough_numbered_inventory_for_all_passengers() {
        assert!(inventory_available("2", 2));
        assert!(!inventory_available("1", 2));
        assert!(inventory_available("有", 3));
    }

    #[test]
    fn masks_order_references_in_notifications() {
        assert_eq!(masked_reference("E123456789"), "****6789");
        assert_eq!(masked_reference("1234"), "****");
    }

    #[test]
    fn new_train_only_task_never_orders_baseline_trains() {
        let task = task_details(NewTrainPolicy::AutoOrder, true);
        let snapshot = TaskQuerySnapshot {
            date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            tickets: vec![ticket("G1"), ticket("G2")],
            new_trains: HashSet::from(["G2".to_string()]),
        };

        let selected = select_task_ticket(&task, &[SeatType::SecondClass], &[snapshot]).unwrap();

        assert_eq!(selected.0.train_no, "G2");
    }

    #[test]
    fn notify_only_policy_excludes_new_train_from_ordering() {
        let task = task_details(NewTrainPolicy::NotifyOnly, false);
        let snapshot = TaskQuerySnapshot {
            date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            tickets: vec![ticket("G2"), ticket("G1")],
            new_trains: HashSet::from(["G2".to_string()]),
        };

        let selected = select_task_ticket(&task, &[SeatType::SecondClass], &[snapshot]).unwrap();

        assert_eq!(selected.0.train_no, "G1");
    }
}
