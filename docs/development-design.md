# 12306-rs 开发设计文档

## 1. 文档目标

本文档基于当前 PRD、目标计划、参考项目分析和 Stitch 原型，定义 `12306-rs` 的首期开发设计。

本文档解决三个问题：

- 后端、CLI、Web UI 如何共享同一套业务能力。
- 抢票、候补、人工验证、待支付等状态如何建模。
- 首期实现时模块、接口、数据库和测试应如何组织。

## 2. 设计边界

首期必须支持：

- Rust 服务。
- Web UI。
- CLI。
- SQLite。
- macOS、Linux、本机运行和 Docker 运行。
- 账号密码登录。
- 人工验证引导。
- 车票查询。
- 多日期、多乘客、多任务。
- 无座。
- 自动提交普通订单到待支付。
- 候补和强候补。
- 候补兑现成功后的待支付提醒。

首期不支持：

- 自动支付。
- 自动破解验证码。
- 自动滑块。
- 风控绕过。
- 高频刷票模式。
- 代理池。
- CDN 查询优化。
- Windows。
- 多账号。
- 分布式任务调度。

## 3. 推荐工程结构

建议使用 Rust workspace：

```text
crates/
  core/             # 领域模型、筛选逻辑、状态机、错误类型
  client_12306/     # 12306 HTTP 客户端、登录、查询、下单、候补
  storage/          # SQLite、migration、repository
  scheduler/        # 任务运行器、轮询、并发、退避
  server/           # Axum Web API、静态资源托管
  cli/              # clap 命令行入口
  web/              # Web UI 源码或静态资源
```

若首期仓库规模较小，也可以先用单 crate 内部模块实现，但模块边界应保持一致：

```text
src/
  core/
  client_12306/
  storage/
  scheduler/
  server/
  cli/
```

## 4. 关键架构原则

- `core` 不依赖 Web、CLI、SQLite 和 12306 HTTP 细节。
- `client_12306` 只封装外部接口，不直接决定任务生命周期。
- `scheduler` 负责任务状态推进和调用顺序。
- `server` 和 `cli` 只调用应用服务层，不复制业务判断。
- SQLite 保存任务、配置、日志、订单和必要会话信息，不保存明文密码。
- 所有自动操作必须写入任务日志，方便用户追踪系统行为。

## 5. 领域模型

### 5.1 Station

```rust
pub struct Station {
    pub name: String,
    pub code: String,
}
```

### 5.2 Passenger

```rust
pub struct Passenger {
    pub id: PassengerId,
    pub name: String,
    pub id_masked: String,
    pub passenger_type: PassengerType,
}
```

首期只在 UI 和日志中展示脱敏证件号。

### 5.3 TicketTask

```rust
pub struct TicketTask {
    pub id: TaskId,
    pub from: Station,
    pub to: Station,
    pub dates: Vec<NaiveDate>,
    pub passengers: Vec<PassengerId>,
    pub seat_preferences: Vec<SeatType>,
    pub accept_no_seat: bool,
    pub train_include: Vec<String>,
    pub train_exclude: Vec<String>,
    pub enable_waitlist: bool,
    pub enable_strong_waitlist: bool,
    pub query_interval_ms: u64,
    pub status: TaskStatus,
}
```

规则：

- `dates` 不能为空。
- `passengers` 不能为空。
- `seat_preferences` 为空时，必须 `accept_no_seat = true` 才能创建任务。
- `query_interval_ms` 最小 1000，默认 3000。
- `enable_strong_waitlist = true` 时，必须同时 `enable_waitlist = true`。

### 5.4 TaskStatus

```rust
pub enum TaskStatus {
    Created,
    Running,
    Querying,
    WaitingLogin,
    VerificationRequired,
    Paused,
    Submitting,
    PendingPayment,
    CandidateSubmitting,
    CandidateSubmitted,
    CandidatePendingPayment,
    Failed,
    Cancelled,
}
```

状态含义：

- `PendingPayment`: 普通订单提交成功，等待用户支付。
- `CandidateSubmitted`: 候补订单已提交，等待兑现。
- `CandidatePendingPayment`: 候补兑现成功，等待用户支付。
- `VerificationRequired`: 登录或提交链路被 12306 人工验证阻塞。

### 5.5 OrderState

```rust
pub enum OrderState {
    None,
    Submitting,
    PendingPayment,
    Expired,
    Cancelled,
}
```

系统只推进到 `PendingPayment`，不执行支付。

### 5.6 WaitlistState

```rust
pub enum WaitlistState {
    None,
    Submitting,
    Submitted,
    FulfilledPendingPayment,
    Failed,
    Cancelled,
}
```

`FulfilledPendingPayment` 对应原型中的“候补兑现成功待支付”。

## 6. 状态机

### 6.1 普通订单状态流

```text
Created
  -> Running
  -> Querying
  -> Submitting
  -> PendingPayment
```

异常分支：

```text
Querying -> WaitingLogin
Querying -> VerificationRequired
Querying -> Failed
Submitting -> Querying
Submitting -> PendingPayment
Submitting -> Failed
```

规则：

- 进入 `PendingPayment` 后不继续轮询同一任务。
- 待支付状态只提供提醒和跳转，不自动支付。
- 提交前必须重新校验日期、车次、席别、乘客和无座配置。

### 6.2 强候补状态流

```text
Running
  -> Querying
  -> CandidateSubmitting
  -> CandidateSubmitted
  -> CandidatePendingPayment
```

触发条件：

- 用户开启候补。
- 用户开启强候补。
- 当前查询批次没有符合任务条件的余票。
- 登录态有效。

规则：

- 未开启候补时，不允许提交候补。
- 开启强候补时，无票后优先提交候补，并暂停继续刷普通余票。
- 候补提交失败要记录原因，再根据错误类型决定回到查询、暂停或失败。
- 候补兑现成功后提醒用户手动支付。

### 6.3 登录验证状态流

```text
LoggedOut
  -> LoggingIn
  -> LoggedIn
```

异常分支：

```text
LoggingIn -> VerificationRequired
VerificationRequired -> LoggedIn
LoggedIn -> Expired
Expired -> LoggingIn
```

规则：

- Web UI 提供人工验证入口。
- CLI 遇到 `VerificationRequired` 时提示用户打开 Web UI。
- 人工验证文案必须明确是打开 12306 官方验证页面，由用户手动完成。

## 7. 调度设计

### 7.1 TaskRunner

每个运行中任务由一个独立 async runner 管理。

职责：

- 读取任务配置。
- 检查登录态。
- 按日期和车次策略查询。
- 过滤符合条件的候选票。
- 推进普通下单或候补。
- 写入任务日志。
- 响应暂停、取消、服务关闭信号。

### 7.2 并发控制

首期不做分布式，只在单进程内控制：

- 多任务可以并发运行。
- 同一个任务同一时间只有一个 runner。
- 全局 12306 请求需要限速。
- 单任务查询间隔最小 1000ms，默认 3000ms。

### 7.3 重试与退避

建议策略：

- 网络超时：短暂退避后重试。
- 12306 限流或风控提示：增加退避时间并记录 warning。
- 登录过期：任务进入 `WaitingLogin` 或 `VerificationRequired`。
- 提交失败：根据错误类型决定回到查询或进入失败。

## 8. 12306 客户端设计

### 8.1 接口边界

```rust
pub trait RailwayClient {
    async fn login(&self, account: LoginRequest) -> Result<LoginResult>;
    async fn check_session(&self) -> Result<SessionState>;
    async fn query_tickets(&self, query: TicketQuery) -> Result<Vec<TicketCandidate>>;
    async fn submit_order(&self, order: SubmitOrderRequest) -> Result<SubmitOrderResult>;
    async fn submit_waitlist(&self, request: WaitlistRequest) -> Result<WaitlistResult>;
}
```

### 8.2 Smoke Test 策略

实现关键链路前做最小 smoke test：

- 查询：确认站点、日期、席别和余票结构可解析。
- 登录：确认成功、失败、需要人工验证三类状态可识别。
- 普通下单：确认提交前检查和提交响应关键字段可识别。
- 候补：确认候补 token、乘客校验、提交结果关键字段可识别。

smoke test 不做大规模逆向，不追求覆盖所有边缘字段。

## 9. SQLite 设计

### 9.1 app_settings

保存本机服务配置。

字段建议：

- `key text primary key`
- `value text not null`
- `updated_at text not null`

### 9.2 sessions

保存登录会话必要信息。

字段建议：

- `id text primary key`
- `state text not null`
- `cookies_json text`
- `csrf_token text`
- `expires_at text`
- `created_at text not null`
- `updated_at text not null`

不保存明文密码。

### 9.3 passengers

字段建议：

- `id text primary key`
- `name text not null`
- `id_masked text not null`
- `passenger_type text not null`
- `raw_ref text`
- `created_at text not null`
- `updated_at text not null`

### 9.4 ticket_tasks

字段建议：

- `id text primary key`
- `from_name text not null`
- `from_code text not null`
- `to_name text not null`
- `to_code text not null`
- `accept_no_seat integer not null`
- `enable_waitlist integer not null`
- `enable_strong_waitlist integer not null`
- `query_interval_ms integer not null`
- `status text not null`
- `remark text`
- `created_at text not null`
- `updated_at text not null`

### 9.5 task_dates

- `task_id text not null`
- `travel_date text not null`
- `priority integer not null`

### 9.6 task_passengers

- `task_id text not null`
- `passenger_id text not null`
- `priority integer not null`

### 9.7 task_seat_filters

- `task_id text not null`
- `seat_type text not null`
- `priority integer not null`

### 9.8 task_train_filters

- `task_id text not null`
- `filter_type text not null`
- `train_no text not null`

`filter_type` 取值：

- `include`
- `exclude`

### 9.9 task_logs

- `id text primary key`
- `task_id text not null`
- `level text not null`
- `event text not null`
- `message text not null`
- `context_json text`
- `created_at text not null`

### 9.10 orders

- `id text primary key`
- `task_id text not null`
- `order_no text`
- `train_no text not null`
- `travel_date text not null`
- `seat_type text not null`
- `state text not null`
- `pay_deadline text`
- `created_at text not null`
- `updated_at text not null`

### 9.11 standby_orders

- `id text primary key`
- `task_id text not null`
- `standby_no text`
- `state text not null`
- `queue_position integer`
- `pay_deadline text`
- `created_at text not null`
- `updated_at text not null`

## 10. Web API 设计

### 10.1 Session API

```text
POST   /api/session/login
POST   /api/session/logout
GET    /api/session/status
POST   /api/session/verification/open
POST   /api/session/verification/complete
```

### 10.2 Ticket API

```text
GET /api/tickets/query?from=SHH&to=BJP&date=2026-07-10
```

返回内容应包含：

- 车次。
- 出发站、到达站。
- 出发时间、到达时间、历时。
- 席别余票。
- 无座状态。
- 候补状态。

### 10.3 Task API

```text
POST   /api/tasks
GET    /api/tasks
GET    /api/tasks/:id
POST   /api/tasks/:id/start
POST   /api/tasks/:id/pause
POST   /api/tasks/:id/resume
POST   /api/tasks/:id/cancel
GET    /api/tasks/:id/logs
```

### 10.4 Settings API

```text
GET /api/settings
PUT /api/settings
```

## 11. CLI 设计

首期命令：

```text
12306-rs serve
12306-rs login
12306-rs logout
12306-rs status
12306-rs query --from 上海 --to 北京 --date 2026-07-10
12306-rs task create
12306-rs task list
12306-rs task show <task-id>
12306-rs task start <task-id>
12306-rs task pause <task-id>
12306-rs task resume <task-id>
12306-rs task cancel <task-id>
12306-rs task logs <task-id>
```

CLI 输出要求：

- 默认输出人类可读文本。
- 后续可增加 `--json`。
- 遇到人工验证时输出 Web UI 地址。
- 不输出密码、完整 Cookie 或敏感 token。

## 12. Web UI 页面映射

### 12.1 总览仪表盘

展示：

- 普通订单待支付。
- 需人工验证。
- 候补已提交等待兑现。
- 关键提醒。
- 最近命中车次。
- Warning 日志。
- 当前监控任务。

### 12.2 车票查询页

展示：

- 出发站、到达站、日期。
- 接受无座。
- 显示候补机会。
- 车次结果表。
- 创建抢票任务入口。

### 12.3 创建任务页

展示：

- 基础路线和多日期。
- 多乘客。
- 席别优先级。
- 无座兜底。
- 车次 include/exclude。
- 候补开关。
- 强候补开关。
- 查询间隔。
- 任务配置摘要。
- 支付提示。

约束：

- 查询间隔最小 1000ms。
- 默认值 3000ms。
- 开启强候补时必须开启候补。

### 12.4 任务列表页

展示：

- 任务 ID。
- 路线。
- 日期。
- 乘客。
- 状态。
- 主要操作。

状态标签必须来自后端状态映射。

### 12.5 任务详情页

展示：

- 任务配置摘要。
- 当前状态。
- 待支付横幅。
- 强候补核心状态。
- 决策链。
- 实时日志。
- 支付行动入口。

候补兑现成功时统一展示：

- `候补兑现成功待支付`
- `待支付 - 候补兑现成功`

### 12.6 登录与人工验证页

展示：

- 账号密码登录。
- 登录状态。
- 受阻任务列表。
- 人工验证辅助入口。

验证文案：

```text
打开 12306 官方验证页面，由用户手动完成验证。
```

### 12.7 系统设置页

展示：

- 服务监听地址。
- 服务端口。
- SQLite 路径。
- 默认查询间隔。
- 日志级别。
- 后续增强能力占位。
- 安全与合规须知。

后续增强开关首期应禁用。

## 13. 筛选与提交策略

### 13.1 车票筛选顺序

1. 日期匹配。
2. 出发站、到达站匹配。
3. 车次 include/exclude 匹配。
4. 席别优先级匹配。
5. 无座兜底匹配。
6. 乘客数量与可用席位匹配。

### 13.2 普通订单提交策略

命中候选票后：

1. 再次确认任务仍处于可提交状态。
2. 再次确认登录态有效。
3. 再次匹配日期、车次、席别、乘客和无座配置。
4. 写入 `Submitting` 状态。
5. 调用 12306 普通下单链路。
6. 成功后写入 `PendingPayment` 和支付提醒日志。

### 13.3 强候补提交策略

无符合条件余票后：

1. 检查 `enable_waitlist`。
2. 检查 `enable_strong_waitlist`。
3. 写入 `CandidateSubmitting` 状态。
4. 调用候补链路。
5. 成功后写入 `CandidateSubmitted`。
6. 候补兑现后写入 `CandidatePendingPayment`。

## 14. 配置默认值

```text
host = "127.0.0.1"
port = 12306
database_path = "./data/12306-rs.sqlite"
query_interval_ms = 3000
min_query_interval_ms = 1000
log_level = "info"
```

## 15. 测试计划

### 15.1 单元测试

- 任务配置校验。
- 车次 include/exclude。
- 席别优先级。
- 无座兜底。
- 强候补触发条件。
- 状态机合法流转。

### 15.2 集成测试

- SQLite migration。
- repository 读写。
- CLI 参数解析。
- Web API 状态返回。
- scheduler 使用 mock `RailwayClient` 推进状态。

### 15.3 Smoke Test

- 查询链路。
- 登录链路。
- 普通下单链路。
- 候补链路。

Smoke test 需要可手动运行，不应在默认 CI 中真实提交订单。

## 16. 实施顺序

建议顺序：

1. 建立 workspace、配置、日志、错误类型。
2. 建立 `core` 领域模型和状态机测试。
3. 建立 SQLite migration 和 repository。
4. 建立 Web API 空壳和 CLI 空壳。
5. 实现查询链路和查询页面。
6. 实现登录与人工验证状态。
7. 实现任务创建、列表、详情和日志。
8. 实现 scheduler 和 mock client 测试。
9. 接入普通下单链路。
10. 接入候补和强候补链路。
11. 完善 Docker、本机运行说明和验收测试。

## 17. 当前原型遗留注意事项

最新原型整体已经可作为开发蓝本，但创建任务页仍需要在实现时强制修正：

- 查询间隔 slider 最小值必须为 1000ms。
- 不展示 500ms 作为可选项。
- 推荐值统一为 3000ms。

实现时以后端配置校验为准，即使前端控件传入低于 1000ms 的值，也必须拒绝或修正。
