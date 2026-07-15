# 参考项目梳理

## 1. 参考项目

当前参考项目：

- `/Users/liqing/Documents/itData/code/python/github/12306`
- `/Users/liqing/Documents/itData/code/python/github/py12306`

这两个项目只作为流程、模型和接口链路参考，不作为 Rust 版代码实现来源。

## 2. 总体结论

两个项目都有参考价值，但价值点不同：

- `12306` 更适合参考 12306 接口链路，尤其是普通下单和候补流程。
- `py12306` 更适合参考产品结构、任务调度、多日期、多任务、Web 管理、Docker 和日志形态。

Rust 版应重新设计实现：

- 使用结构化类型替代魔法数字和散落字符串。
- 使用状态机表达登录、查询、普通下单、候补。
- 使用 SQLite 保存配置、任务、状态和日志。
- 使用 Web UI 引导人工验证，不照搬自动打码或自动滑块处理。

## 3. 已确认参考边界

- 默认先认为参考项目里的 12306 接口链路可用，以减少前期探索时间。
- 查询、登录、普通下单、候补四类关键链路实现前，需要做最小 smoke test。
- smoke test 只验证接口能返回可识别结构，不做大规模逆向。
- 自动打码、自动滑块、代理、CDN、更激进查询策略放入后续增强。
- 后续增强必须由用户显式开启。
- 不做分布式。
- 不做多账号协同抢票。
- 参考项目中的敏感配置暂不处理，但不得复制到本项目。

## 4. `12306` 项目参考价值

路径：

- `/Users/liqing/Documents/itData/code/python/github/12306`

主要价值：

- 12306 URL 和请求参数集中配置。
- 查询余票后提取 `secretStr`、车次、席别、余票、候补信息的流程。
- 普通订单链路：提交预订、校验订单、查队列、确认排队、查询订单结果。
- 候补链路：候补资格检查、成功率查询、候补提交、候补订单确认、候补排队查询。
- 乘客提交字符串和候补乘客信息拼装方式。

重点参考文件：

- `config/urlConf.py`: 接口地址和请求配置。
- `inter/Query.py`: 余票查询、普通票命中、候补机会判断。
- `inter/SubmitOrderRequest.py`: 普通订单和候补订单提交入口。
- `inter/CheckOrderInfo.py`: 普通订单校验。
- `inter/GetQueueCount.py`: 普通订单排队和候补排队。
- `inter/ConfirmSingleForQueue.py`: 普通订单确认排队。
- `inter/QueryOrderWaitTime.py`: 普通订单等待结果。
- `inter/ChechFace.py`: 候补前身份状态检查。
- `inter/GetSuccessRate.py`: 候补成功率查询。
- `inter/PassengerInitApi.py`: 候补订单信息初始化。
- `inter/ConfirmHB.py`: 候补订单确认。
- `inter/GetPassengerDTOs.py`: 乘客信息和提交字符串生成。

不建议照搬：

- 自动打码。
- 云打码。
- 代理切换。
- CDN 查询。
- 高频请求参数。
- Python 全局配置。
- 明文账号密码配置。
- Python 2 兼容代码。

## 5. `py12306` 项目参考价值

路径：

- `/Users/liqing/Documents/itData/code/python/github/py12306`

主要价值：

- 多日期、多任务、多乘客任务模型。
- 任务字段设计：日期、站点、乘客、席别、车次白名单、车次黑名单、时间段、查询间隔。
- 查询任务循环、随机间隔、失败后增加等待时间。
- 普通订单状态链。
- Web 管理页面的信息结构。
- Docker 运行方式。
- 日志分类和实时日志查看。

重点参考文件：

- `py12306/query/job.py`: 任务模型、查询循环、筛选逻辑。
- `py12306/query/query.py`: 查询调度。
- `py12306/order/order.py`: 普通订单链路。
- `py12306/user/job.py`: 用户登录态和心跳。
- `py12306/helpers/api.py`: 接口常量。
- `py12306/helpers/type.py`: 席别和订单席别映射。
- `py12306/web/web.py`: Web 管理入口。
- `py12306/web/handler/*.py`: Web API 形态。
- `Dockerfile`: Docker 运行方式。
- `docker-compose.yml.example`: Docker Compose 运行方式。

不建议照搬：

- Redis 分布式。
- 多账号协同。
- 自动滑块处理。
- 外部通知全量实现。
- 直接执行 Python 配置文件。
- 旧前端静态资源。

## 6. 可吸收设计

### 6.1 任务配置字段

建议 Rust 版任务模型至少包含：

- 任务名称。
- 出发站。
- 到达站。
- 出行日期列表。
- 乘客列表。
- 席别列表。
- 是否允许无座。
- 车次白名单。
- 车次黑名单。
- 出发时间段。
- 是否开启候补。
- 是否开启强候补。
- 查询间隔。
- 任务状态。

### 6.2 查询筛选规则

建议吸收：

- 多日期依次查询。
- 席别按用户配置顺序优先匹配。
- 车次白名单优先于全量车次。
- 车次黑名单用于排除不接受车次。
- 无座必须用户显式允许。
- 余票不足乘客人数时，默认不提交。

暂不吸收：

- 余票不足时自动减少乘客提交。
- 多站点扩展购票。
- 高频刷新模式。

### 6.3 普通订单状态机

建议 Rust 版表达为：

1. `submit_order_request`
2. `init_confirm_page`
3. `check_order_info`
4. `get_queue_count`
5. `confirm_single_for_queue`
6. `query_order_wait_time`
7. `pending_payment`

如果遇到额外验证，进入 `verification_required`，由 Web UI 引导用户处理。

### 6.4 候补状态机

建议 Rust 版表达为：

1. `standby_candidate_found`
2. `check_standby_face_or_identity`
3. `get_standby_success_rate`
4. `submit_standby_order_request`
5. `init_standby_passenger`
6. `confirm_standby_order`
7. `query_standby_queue`
8. `standby_submitted`

强候补规则：

- 只有用户显式开启候补和强候补时才启用。
- 没有符合条件余票时，候补优先于继续刷票。
- 候补成功或失败都要记录任务日志。

## 7. 后续增强

后续可以考虑：

- CDN 查询优化。
- 用户自配置代理。
- 更细的查询间隔策略。
- 验证辅助适配器。
- 飞书以外的外部消息通知。

后续增强约束：

- 不进入 MVP。
- 不默认开启。
- 不影响普通查询、普通下单、候补的核心链路。
- 不写成验证码破解或风控绕过。
- 不实现分布式，除非后续重新确认需求。

## 8. 实现前验证策略

每个关键链路实现前做最小 smoke test：

- 查询: 指定日期、出发站、到达站，确认返回余票列表结构。
- 登录: 账号密码提交后，确认可识别成功、失败、需要人工验证三类状态。
- 普通下单: 在受控条件下确认普通下单链路关键响应结构。
- 候补: 在受控条件下确认候补链路关键响应结构。

验证失败时：

- 不扩大探索范围。
- 记录接口差异。
- 只调整当前模块需要的最小实现。
- 不阻塞无关模块开发。

## 9. 风险提醒

- 参考项目年代较旧，接口字段可能已经变化。
- 参考项目包含本项目首期不做的能力，不能直接照搬。
- 参考项目存在本地未提交改动，后续对比时应注意文件状态。
- 参考项目可能包含敏感配置，本项目不得复制任何账号、密码、Cookie 或 Token。
