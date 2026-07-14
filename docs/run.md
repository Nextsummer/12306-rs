# 运行说明

## 本机运行

```bash
cargo run -p rs12306-cli -- serve --host 127.0.0.1 --port 12306
```

访问 `http://127.0.0.1:12306`。

也可以使用 Makefile：

```bash
make deploy
make package
make package-linux
make run HOST=127.0.0.1 PORT=12306
```

`make deploy` 更新根目录二进制，`make package` 在 `dist/` 生成当前系统和架构的发布包。
`make package-linux` 默认生成 Linux amd64 静态包；也可以使用 `make package-linux-arm64` 生成 Linux arm64 静态包，需要 Docker Buildx。静态包不依赖目标机器的 glibc 版本。

## CLI 小版本

当前 CLI 小版本通过 12306 真实余票接口查票，`buy` 会基于真实余票结果匹配车次和席别，并调用 12306 普通订单接口提交订单。系统不自动支付，下单成功后只提示用户去 12306 官方渠道手动支付。

构建 release 二进制：

```bash
cargo build --release -p rs12306-cli
```

二进制路径：

```bash
target/release/12306-rs
```

用 release 二进制执行：

```bash
target/release/12306-rs query --from SHH --to BJP --date 2026-07-10

target/release/12306-rs login --qr

target/release/12306-rs login --username <12306账号>

target/release/12306-rs login --username <12306账号> --id-last4 <证件后4位>

target/release/12306-rs login --username <12306账号> --sms-code <短信验证码>

target/release/12306-rs passenger 12306-list

target/release/12306-rs passenger add --name 张三 --id-masked '310************1234'

target/release/12306-rs buy \
  --from SHH \
  --to BJP \
  --date 2026-07-10 \
  --train G11 \
  --seat second_class \
  --choose-seats 1A \
  --at '2026-07-09 14:30:00' \
  --yes \
  --passenger-id 00000000-0000-4000-8000-000000000001
```

开发期也可以用 `cargo run`：

```bash
cargo run -p rs12306-cli -- query --from SHH --to BJP --date 2026-07-10

cargo run -p rs12306-cli -- login --qr

cargo run -p rs12306-cli -- login --username <12306账号>

cargo run -p rs12306-cli -- login --username <12306账号> --id-last4 <证件后4位>

cargo run -p rs12306-cli -- login --username <12306账号> --sms-code <短信验证码>

cargo run -p rs12306-cli -- passenger 12306-list

cargo run -p rs12306-cli -- passenger add --name 张三 --id-masked '310************1234'

cargo run -p rs12306-cli -- buy \
  --from SHH \
  --to BJP \
  --date 2026-07-10 \
  --train G11 \
  --seat second_class \
  --choose-seats 1A \
  --at '2026-07-09 14:30:00' \
  --yes \
  --passenger-id 00000000-0000-4000-8000-000000000001
```

推荐使用 `login --qr` 进行官方二维码登录，命令会在终端显示二维码，并在当前目录保存 `12306-login-qr.png` 作为兜底；二维码过期会自动刷新，默认最多等待 600 秒。使用 12306 App 扫码确认后会把会话保存到 SQLite。账号密码登录默认使用隐藏式密码输入，也可以通过 `RS12306_PASSWORD` 提供密码；不建议把密码直接写入命令行。若 12306 要求验证码、滑块或其他核验，命令会进入 `verification_required`；此时请改用 `login --qr`。`login --verified` 仅保留给本地开发调试。
如果 12306 要求短信验证，可以先用 `login --username <账号> --id-last4 <证件后4位>` 获取短信验证码，再用 `login --username <账号> --sms-code <验证码>` 完成登录，密码会隐藏输入。

真实下单前需要先添加本地乘客，`--name` 必须和 12306 常用联系人姓名一致；完整证件号和手机号会在下单时从 12306 常用联系人接口读取，本地只保存脱敏证件号用于展示。
可以用 `passenger 12306-list` 查看当前登录账号的 12306 常用联系人列表；命令会把联系人同步到本地并显示可直接用于 `--passenger-id` 的 UUID，证件号和手机号仍保持脱敏。
高铁/动车可用 `--choose-seats` 传座位位置偏好，例如 `1A`、`1F`、多人 `1A1F`；A/F 通常是靠窗，C/D 通常是过道，B 是中间位。卧铺上中下铺暂不接，避免传错 12306 铺别参数导致下单异常。
`buy` 必须明确指定 `--train`，即时下单会在提交真实订单前展示车次、日期、席别和乘客并请求确认；使用 `--yes` 可跳过确认。定时下单可用 `--at`，支持 `2026-07-09 14:30:00`、`2026-07-09T14:30:00`、`14:30:00`、`14:30`，定时自动提交必须同时传 `--yes`。命令会提前预热一次查询，但仍是本机前台等待，终端进程退出后定时失效。
普通订单排队时会每 3 秒打印 `queue: wait_time=...s wait_count=...`，避免定时放票高峰看起来像卡死。
`task start <任务ID>` 和 `task resume <任务ID>` 会在前台持续查询并在命中后提交普通订单；可从另一个终端执行 `task pause` 或 `task cancel`。多个任务可分别由多个 CLI 进程运行。任务同时开启候补和强候补后，无符合条件余票但存在匹配候补机会时，会优先提交候补并停止继续刷票；候补提交后请前往 12306 官方渠道检查确认或支付状态。

新增车次监控通过任务参数启用：`--new-train-policy notify_only` 只通知新增车次，`--new-train-policy auto_order` 允许符合原任务条件的新增车次进入下单和候补流程。增加 `--new-trains-only` 后任务首次查询按日期建立基线，之后只处理新增车次；独立 `notify_only` 任务不要求乘客或 12306 登录。查询间隔仍由 `--query-interval-ms` 控制并执行最小间隔限制。

## 飞书通知

```bash
target/release/12306-rs notify types
target/release/12306-rs notify set feishu
target/release/12306-rs notify test feishu
target/release/12306-rs notify disable feishu
target/release/12306-rs notify enable feishu
target/release/12306-rs notify remove feishu
```

`notify set feishu` 会隐藏式读取飞书群机器人 Webhook，并把全局配置保存到当前 SQLite。普通订单成功、候补提交成功和 12306 最终提交错误会发送通知；通知失败重试两次，仍失败时只记录终端和任务日志，不影响订单状态。
`notify types` 会显示通知类型、配置状态、启用状态和脱敏后的 Webhook，便于核对当前配置；完整有效性可通过 `notify test feishu` 验证。

## Docker 运行

```bash
docker build -t 12306-rs .
docker run --rm -p 12306:12306 -v 12306-rs-data:/data 12306-rs
```

容器默认使用 `/data/12306-rs.sqlite` 保存 SQLite 数据，监听 `0.0.0.0:12306`。

也可以快速部署：

```bash
make docker-deploy PORT=12306
make docker-logs
make docker-stop
```
