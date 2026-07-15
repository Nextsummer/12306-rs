# 12306-rs

`12306-rs` 是一个使用 Rust 开发的 12306 余票查询与购票任务服务。CLI 是当前主要可用入口，Web UI 仍在开发中，数据存储在本机 SQLite。

项目可以查询真实余票、登录 12306、读取常用联系人、提交普通订单与候补订单。系统不会自动支付；订单提交后需要前往 12306 官方 App 或网站确认并完成支付。

> 本项目不是 12306 官方软件。请合理设置查询间隔，遵守 12306 的服务规则。真实下单和候补会影响账号中的订单，请先使用测试行程谨慎验证。

## 当前能力

- 真实余票查询，支持中文站名、拼音和车站代码。
- 二维码登录、账号密码登录和短信验证。
- 终端二维码展示，二维码过期自动刷新。
- 同步 12306 常用联系人，本地仅展示脱敏身份信息。
- 普通订单提交、排队进度输出和待支付提醒。
- 指定车次、席别、多乘客和高铁/动车座位偏好。
- SQLite 持久化定时下单，服务重启后可恢复尚未到时的任务。
- 多日期、多席别、多车次过滤的抢票任务。
- 新增车次基线监控、增开/放票通知和自动下单。
- 无座、候补和强候补。
- 任务状态、日志、普通订单和候补订单持久化。
- 飞书群机器人购票、候补和提交失败通知。
- macOS、Linux 和 Docker 运行；Web UI 目前仅完成部分页面和 API 接入。

暂不支持自动支付、验证码破解、滑块绕过、卧铺上中下铺选择、Windows、分布式调度和多账号。

## 环境要求

- Rust 1.96 或更高版本。
- macOS 或 Linux。
- 可以访问 `https://kyfw.12306.cn`。

## 构建

```bash
cargo build --release -p rs12306-cli
```

生成的程序位于：

```text
target/release/12306-rs
```

仓库根目录也保留了一份已构建的 `./12306-rs`。

### 使用 Makefile

直接运行 `make` 查看所有快捷命令：

```bash
make
make check
make test
make deploy
make package
make package-linux
```

`make deploy` 会构建 release 并更新仓库根目录的 `./12306-rs`。`make package` 会在 `dist/` 下生成包含二进制、README 和运行说明的压缩包。

在 macOS 或 Linux 上通过 Docker Buildx 生成 Linux 二进制包：

```bash
make package-linux          # 默认 linux/amd64
make package-linux-amd64
make package-linux-arm64
```

Linux 包同样输出到 `dist/`。该方式在 Docker 的 Alpine/musl 构建环境内生成静态链接二进制，不依赖目标机器的 glibc 版本，也不要求本机安装 Linux 交叉编译工具链。

本机启动和 Docker 部署：

```bash
make run HOST=127.0.0.1 PORT=12306
make docker-deploy PORT=12306
make docker-logs
make docker-stop
```

## Web UI

> **未完成：** Web UI 当前已接入真实查询、任务配置、后台启动和状态查看，但部分页面与交互仍处于原型阶段。CLI 仍是功能最完整的入口。

```bash
./12306-rs serve --host 127.0.0.1 --port 12306
```

浏览器访问 [http://127.0.0.1:12306](http://127.0.0.1:12306)。

## CLI 快速开始

### 1. 登录

推荐使用二维码登录：

```bash
./12306-rs login --qr
```

账号登录默认隐藏密码输入：

```bash
./12306-rs login --username <12306账号>
```

如果需要短信验证：

```bash
./12306-rs login --username <12306账号> --id-last4 <证件后4位>
./12306-rs login --username <12306账号> --sms-code <短信验证码>
```

查看本地状态并验证 12306 会话：

```bash
./12306-rs status
```

### 2. 同步乘车人

```bash
./12306-rs passenger 12306-list
```

输出中的“本地ID”用于后续 `--passenger-id`。同名联系人会通过脱敏证件号再次匹配。

### 3. 查询余票

```bash
./12306-rs query --from 上海 --to 嘉兴 --date <YYYY-MM-DD>
```

### 4. 即时下单

```bash
./12306-rs buy \
  --from 上海 \
  --to 嘉兴 \
  --date <YYYY-MM-DD> \
  --train <车次> \
  --seat second_class \
  --choose-seats 1A \
  --passenger-id <本地乘车人ID>
```

命令会在提交真实订单前显示车次、日期、席别和乘车人，并要求确认。使用 `--yes` 可以跳过确认。

### 5. 定时下单

```bash
./12306-rs buy \
  --from 上海 \
  --to 嘉兴 \
  --date <YYYY-MM-DD> \
  --train <车次> \
  --seat second_class \
  --at '<YYYY-MM-DD HH:MM:SS>' \
  --yes \
  --passenger-id <本地乘车人ID>
```

使用 `--at` 时会先把任务和启动时间保存到 SQLite，再在当前终端等待执行；为避免误操作，必须同时传入 `--yes`。等待进程中断后，启动 `12306-rs serve` 会恢复处于运行或查询状态的任务。

## 抢票任务

创建任务：

```bash
./12306-rs task create \
  --from-name 上海 \
  --from-code SHH \
  --to-name 嘉兴 \
  --to-code JXH \
  --date <YYYY-MM-DD> \
  --seat second_class \
  --include-train <车次> \
  --seat-position A \
  --enable-waitlist \
  --enable-strong-waitlist \
  --passenger-id <本地乘车人ID>
```

启动任务：

```bash
./12306-rs task start <任务ID>
```

`task start` 和 `task resume` 会在当前终端持续运行。暂停、恢复、取消或查看日志可以在另一个终端执行：

```bash
./12306-rs task pause <任务ID>
./12306-rs task resume <任务ID>
./12306-rs task cancel <任务ID>
./12306-rs task show <任务ID>
./12306-rs task logs <任务ID>
```

通过 Web API 或 Web UI 启动的任务由 `serve` 在后台运行；服务重启后会自动恢复 `running` 和 `querying` 任务。任务可使用 `--start-at` 持久化启动时间，使用可重复的 `--seat-position` 为每位乘客选择座位位置，例如两位乘客传 `--seat-position A --seat-position F`。未启动的任务可用 `task edit <任务ID> <完整配置>` 修改。

如果下单请求超时导致结果未知，任务会进入 `reconciliation_required`，不会自动重复提交。请先在官方 12306 核对订单；确认没有生成订单后再执行：

```bash
./12306-rs task reconcile <任务ID> --confirmed-no-order
```

多个任务可以分别由多个 CLI 进程运行，也可以由一个 `serve` 进程管理多个后台 worker。开启强候补后，无符合条件余票但存在匹配候补机会时，系统会优先提交候补并停止继续刷票。候补提交接口已接入；候补长期兑现状态尚未自动同步，请在官方 12306 中核对。

监控普通任务中新出现的匹配车次，并允许自动下单：

```bash
./12306-rs task create <其他参数> --new-train-policy auto_order
```

创建只监控新增车次、仅发送通知且不需要乘客的独立任务：

```bash
./12306-rs task create \
  --from-name 上海 --from-code SHH \
  --to-name 嘉兴 --to-code JXH \
  --date <YYYY-MM-DD> \
  --seat second_class \
  --new-trains-only \
  --new-train-policy notify_only
```

`--new-train-policy` 支持 `off`、`notify_only` 和 `auto_order`。每个日期首次成功查询只建立基线；之后发现新增车次时通知一次，首次出现符合配置的余票时再通知一次。`task show <任务ID>` 可以查看已发现的新增车次。

## 飞书通知

通知配置保存在当前 SQLite 中，是所有登录账号和任务共用的全局配置。当前支持 `feishu`：

```bash
# 查看支持的类型、是否配置、是否启用和脱敏后的配置信息
./12306-rs notify types

# 隐藏式输入飞书群机器人 Webhook，保存后自动启用
./12306-rs notify set feishu

./12306-rs notify test feishu
./12306-rs notify disable feishu
./12306-rs notify enable feishu
./12306-rs notify remove feishu
```

新增车次、增开车次首次放票、普通订单创建成功、候补提交成功以及 12306 返回最终提交错误时会通知所有已启用类型。临时余票查询错误不会发送，避免频繁刷群。通知失败会额外重试两次并写入终端和任务日志，但不会改变订单状态。

## 席别与选座

`--seat` 支持：

- `business`
- `first_class`
- `second_class`
- `soft_sleeper`
- `hard_sleeper`
- `hard_seat`
- `no_seat`

高铁和动车选座使用 `--choose-seats`，例如单人 `1A`、两人 `1A1F`。选座数量必须和乘客数量一致。卧铺上中下铺暂不支持选择。

## Docker

```bash
docker build -t 12306-rs .
docker run --rm \
  -p 127.0.0.1:12306:12306 \
  -e RS12306_API_TOKEN='<至少16位随机字符串>' \
  -v 12306-rs-data:/data \
  12306-rs
```

服务默认只允许无 token 的本机回环监听。监听非回环地址（Docker 容器内默认为 `0.0.0.0`）时必须配置至少 16 位 API token。远程使用 Web UI 时，在浏览器控制台设置 `localStorage.setItem('rs12306_api_token', '<token>')` 后刷新页面；`/api/health` 不要求 token。

容器监听 `0.0.0.0:12306`，SQLite 数据保存在 `/data/12306-rs.sqlite`。

## 数据与安全

- 默认数据库：`./data/12306-rs.sqlite`。
- 可通过 `--database` 或 `RS12306_DATABASE` 修改路径。
- SQLite 和二维码图片在 macOS/Linux 上使用 `0600` 权限。
- 不保存账号密码；登录 Cookie 会保存在 SQLite 中。
- 飞书 Webhook 会保存在 SQLite 中；`notify types` 只显示脱敏地址，日志不会输出完整内容。
- 可通过 `RS12306_PASSWORD` 提供密码，但更推荐使用隐藏式交互输入或二维码登录。

## 工程结构

```text
crates/
  cli/             命令行入口与前台任务运行器
  client_12306/    12306 登录、乘车人、普通订单与候补接口
  core/            领域模型与任务状态机
  scheduler/       任务筛选和策略决策
  server/          Axum Web API 与未完成的 Web UI
  storage/         SQLite 存储
```

## 开发验证

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 文档

- [运行说明](docs/run.md)
- [产品需求文档](docs/prd.md)
- [目标计划](docs/target-plan.md)
- [开发设计](docs/development-design.md)
- [参考项目分析](docs/reference-analysis.md)

## License

MIT
