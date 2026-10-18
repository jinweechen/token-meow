# G3 / G4 / G5 最小补验证分析

记录时间：2026-10-15。性质：轻量只读分析，未运行下列验证，未修改生产源码、现有测试、dist 或历史回执。C12-reviewfix 的实际 P95、完整 4.275GB 导入和资源观察由根节点独立执行，本文不改变其结论。

候选依据是 `output/build-candidates/c12-reviewfix-migration-capabilities-history-followup-20261015/build-receipt.json`：应用 SHA `23D9C9C45392A1106796340491FB2AB9679DEC8FB00ACDA64440411F327D6ED3`；同源离线 `ledger_audit.exe` SHA `3973F51C397D8BC9D6375B827D8D6D44A237DE234C14CECF06538AF8522C1EB7`。这里只引用构建回执，没有重新散列全树，也没有启动该候选。构建成功不等于以下功能或 M4 已通过。

## 1. 当前实际已覆盖的范围

| 缺口 | 已有命名测试及准确入口 | 实际断言与不能外推的内容 |
| --- | --- | --- |
| G3 回合结果、提醒 | `src-tauri/src/core/tests.rs:74` `fixture_native_and_cumulative_are_not_added_twice`；`:301` `child_inherits_parent_history_without_reassigning_parent_usage_or_notifying_child`；`:332` `completion_refinement_updates_one_record_and_preserves_read_and_delivery` | C01 固定文件包含未知结束；C13 直接 ingest 合成父子身份并断言没有子提醒；C14 unknown→completed 只有一条、保留已读且不重新 eligible。不是原始 `turn_failed` / `turn_aborted` 经文件 reader 的完整结果组。 |
| G3 通知投递状态 | `tests.rs:828` `notification_claim_is_atomic_preserves_refinement_and_never_replays_on_restart`；`:882` `do_not_disturb_suppresses_live_notification_persistently_instead_of_delaying_it` | 直接 ingest live Context 后领取一次，手动 `finish_notification(...,"shown",...)`，后补与重启不重播；DND 解除不补弹。测试人为写入 shown，不证明 Windows 实际送达。 |
| G3 工具 | `tests.rs:1040` `structured_tool_result_metadata_is_retained_but_body_and_spoofed_stdout_are_not`；`:1289` `paused_history_keeps_old_cursor_but_collects_new_files_and_old_file_tail_without_recount` | C37 的结构化非零退出 2、1.25s→1250ms、正文不入库、stdout 伪造标记不当执行结果；C41 的真实双游标文件链保留退出 7、failed。未包含主动取消、MCP isError、两个调用交错返回的完整组。 |
| G4 预算 | `tests.rs:400` `budget_identity_is_per_rule_version_scope_local_day_and_tier_not_usage_event`；`:1089` `root_task_budget_includes_explicit_children_across_days_and_never_other_source_same_ids`；`:1138` `live_budget_crossing_claims_once_and_cleared_inbox_does_not_refill_same_threshold` | C16 只有同日 threshold 100→200 改版，不是系统跨午夜；C38 根任务累计昨天根响应+今天明确子响应为 240，另一来源同 ID 不混入，thread 当日为 0；C39 真实文件追加跨线领取一次、清 inbox 不回填。保留已有成功，无需重跑。 |
| G4 筛选、活动、计时 | `tests.rs:713` `global_activity_is_unfiltered_counts_root_tasks_and_rejects_future_activity`；`:953` `all_fact_views_honor_project_model_provider_and_root_filters_and_missing_root_usage_is_related`；C01；`src/shared/format.test.ts` 的 `latest turn missing TTFT stays missing instead of borrowing an earlier measurement` | C28 日期/模型把明细筛空仍有 1 个根活动，未来事件变 uncertain；C36 只联合 project+model+provider+root 四条件；C01 1500ms 工具耗时、TTFT 500ms。未独立断言每个筛选维度、上海午夜的实际归日、两个并行回合空 TTFT 不互借。前端纯格式测试不能替生产查询。 |
| G5 清理、导出、取消 | `tests.rs:424` `clear_watermark_prevents_automatic_refill_but_explicit_history_import_is_possible`；`:439` `csv_blocks_formulas_newlines_and_can_anonymize_project`；`:516` `cancellation_does_not_commit_a_partial_record` | C17 清账本后复制不回灌、显式补导恢复 1200；C18 公式/换行安全、真实 Core 导出内容匿名，但没有系统保存框；C22 cancel 在扫描开始前已经 true。没有直接 `apply_retention` 删除/水位回归。 |
| G5 已有中途历史取消补证 | `tests.rs:1387` `cancelled_history_survives_restart_and_requires_explicit_reimport_while_live_remains_active`；C41 | C42 在历史已有部分进度后取消，重启保持 cancelled，拒绝 resume，实时旧文件尾部仍采集，显式 import 才恢复历史；C41 pause/resume 不跳过历史。不能再称“没有任何中途取消验证”。未证明 worker 正读取或提交时正常退出/异常终止的完整恢复。 |
| G5 账户清缓存 | `src-tauri/src/desktop.rs:2280` `account_cache_never_relabels_old_root_or_late_reply_as_current_source`；`src-tauri/src/account_bridge.rs:872` `clearing_cached_data_drops_account_facts_without_switching_source_or_cancel_handle` | 桌面 cache.clear 拒绝旧 generation 的迟到回复，允许新回复；bridge 清账户/用量/身份/backoff，不换 source/cancel。不是用户点击清理到真实迟到 RPC 的完整原生回执。 |

以上是已有断言的清点，不是本次新跑。旧索引的 G3/G4/G5 仍是**具体缺证**，不能按测试名字或数量推导全部验收场景已过，也不能据未跑场景断言产品必然失败。

## 2. 优先可用现成 CLI 补齐的部分

### A. 一份小 JSONL：失败、中断、MCP、取消、并行工具与历史重放

这项不需要改码或重建。使用全新 `output/performance/<唯一case>/source` 和独立 `data`，全部数据由验证器手写、没有正文/凭据，也不调用模型。为避免 30 天默认窗口影响，事件时间选生成样本时的当前时间或其前几秒，固定字节后不再变动。

准确生产链为 `Core::scan` → reader → `adapter.rs:1137` 回合结果分派、`:1240` 工具事件 → `:1380` completion / `:1528` tool_event → SQLite。最小独立 oracle：

1. 四个不同根回合，分别为明确 completed、原始 `turn_failed`、原始 `turn_aborted`、无 status 的 `task_complete`，期望 `completed/failed/interrupted/unknown` 四种结果及四条收件箱事实。失败之后追加同根无明确结果的 generic completion，不能把明确失败覆盖成 unknown 或 success。
2. 两个明确归属上述根的子回合结束，不增加根完成提醒；使用独立 child 文件的 `session_meta.parent_thread_id` 与明确 `root_turn_id`，不从名称推断关系。
3. 同一回合内交错开始/返回三个不同 call_id：命令 exit 0、MCP `result.isError=true`、主动取消 status=`cancelled` 且无 exit_code。期望逐调用保留 completed / failed / cancelled，退出码未知保持 null；其中一项用 `duration:{secs:1,nanos:250000000}` 期望 1250ms。正文哨兵只放在命令参数/输出中，落库数据不含哨兵。
4. 首次历史导入 `notifyEligible=false`，第二次对同一 data 重开、原文件复制再读，回合、工具、收件箱身份和 token 总量均不增加。样本不提供 usage 时，`responseCount/trustedMeteredResponseCount=0`、usage 表为 0 行、`legacyTokens=0`，但 `inputTokens/outputTokens/totalTokens` 保持 null；工具返回不能制造用量事实，不能把未计量总量断言为 0。

现成命令模板如下；`$case` 必须由执行者新建，所有 out 文件名也必须新建，不能覆盖旧回执：

```powershell
$candidate = 'D:\project\宠物\output\build-candidates\c12-reviewfix-migration-capabilities-history-followup-20261015'
$case = 'D:\project\宠物\output\performance\<新建唯一case>'
& "$candidate\ledger_audit.exe" "--source=$case\source" "--data=$case\data" '--scans=30' "--out=$case\first.json"
& "$candidate\ledger_audit.exe" "--source=$case\source" "--data=$case\data" '--scans=30' "--out=$case\reopen.json"
```

CLI 的准确限制见 `src-tauri/examples/ledger_audit.rs:76-106`：它确实调用生产 open/scan/snapshot/shutdown，但输出只有 overview、部分 health、数量与 activity，没有完整 turns/tools/inbox、任意 query、action 或 claim 接口。因此还须对**此私有 data** 用 SQLite `mode=ro` 查询实际事实，例如：

```sql
SELECT turn_id,json_extract(data,'$.status'),json_extract(data,'$.durationMs') FROM turns ORDER BY turn_id;
SELECT json_extract(data,'$.id'),json_extract(data,'$.status'),json_extract(data,'$.exitCode'),json_extract(data,'$.isError'),json_extract(data,'$.durationMs') FROM items ORDER BY id;
SELECT id,json_extract(data,'$.turnId'),json_extract(data,'$.status'),json_extract(data,'$.notifyEligible') FROM inbox_events ORDER BY id;
```

将每个指定 ID 与预先手算 oracle 比较，不能只看行数或程序退出 0。必须记录 fixture 前后摘要、pending/issues、首次及重开结果。此项闭合**文件解析/持久事实/历史幂等**子范围，不能证明实时系统 Toast、提醒动画或真实 Codex 子代理版本支持。

## 3. G4 最小必要补验证：三个有明确断言的 case

这些精确断言优先复用 `tests.rs:7` Rig、`:46` event、`:49` native、`:66` direct 与真实 `Core::snapshot/action`。本轮未新增或运行测试；若以后补为 exact 测试，只需新增受影响 case，不重跑已有 C16/C28/C36/C38/C39。也可在资源观察结束后用冻结 C12 的真实面板完成可操作子范围，但只读 SQL 不能代替 production snapshot 的筛选验证。

| 最小 case | 预先确定的 oracle / 准确入口 | 留下的边界 |
| --- | --- | --- |
| 上海午夜归日 + 空 TTFT + 并行 | 两个 native usage 分别在 `15:59:59.999Z` 和 `16:00:00.000Z`，分别计入上海前/后一天；同一跨午夜回合按完成日进入回合列表，其全回合 duration 不拆成单响应速度。并行第二回合 duration 有值、TTFT 缺失，必须仍 null。查询入口 `queries.rs:5-31`、`:286`、`:587-606`。 | 只改事件时间可证明数据归日，不能声称程序时钟真正经过了午夜。 |
| 独立筛选 + 活动 freshness | 复用 C36 两会话 fixture，分别仅给 project、model、providerId、rootTurnId、sourceId 查询，逐一断言 overview/sessions/turns/tools/inbox 不泄漏；再加一个已有旧格式区间，核对 legacyTokens 同样按 scope 排除且始终独立于 trusted total（`queries.rs:479-514`）。保留现有联合 case 不重测。同 fixture 增加根/明确子回合，分别给 now−601s 和 fresh 事件，检查 uncertain/active 根与子分列；追加 fresh 活动证明恢复，全局 activity 不随日期/模型筛选变化。另一个 native 响应只给 requested_model、不给 turn_id，模型计入所请求模型且不制造回合（`adapter.rs:503-507`、`:1066-1085`）。查询入口 `queries.rs:326-370`、`:556-583`、`:775-783`。 | 这是有界的单维反例，不要求无意义全笛卡尔积；时间戳边界不等于长期真人持续观察；model 不应借前一回合推断。 |
| 日预算周期与两个阈值身份 | 一份 fixture 同时含上海昨日/今日 usage；当日 usedTokens 仅今日，根任务仍跨日累计。用同范围两条规则（例如 100/200）跨线，各领取一次且重放不重复；已有昨日 budgetState 不能占今日 period 身份；已跨线后编辑规则只生成历史/基线，不补弹旧量。入口 `queries.rs:810-889`、`:912-1002`、`core.rs:607-677`。 | 目前一条规则只有一个 threshold（`core.rs:615-619`），两条规则是两个阈值，不能写成内置 80%/100% 多档。真实时钟跨日仍需受控时钟方案或真实午夜回执；不改系统时间，不把事件跨日冒充时钟跨日。 |

固定 CLI 没有预算动作与 query 参数，不能单独完成这三项合同。为补一个时钟边界而擅改冻结生产时钟/核心没有必要；可先取得上述已可证明子范围，真实 clock rollover 如未执行则继续明确未证明。

## 4. G5 优先用冻结 C12 完成的三个独立小场景

必须等根节点性能运行结束后，在另外的私有 source/data 进行，避免污染性能观察或使用真实源/账本。这些动作现有产品入口已经存在，不需要新增通用调试命令。

### B. 留存删除、水位与显式恢复

用两个小 native 响应：A=当前时间前 10 天、总量 120；B=当前、总量 220。两者处于初始最近 30 天/default 90 天范围，先确认总量 340。通过设置页把保留期改为 7 天并应用保留策略，期望只剩 B=220；之后复制旧 A 文件、再次扫描、退出重开，A 不能自动回灌，水位存在且源文件摘要不变。再明确将保留期扩大到 365 天，并从 A 日期执行显式补导，恢复到 340/两响应且重放不增加。

生产入口：`src/features/Panels.tsx:1248-1270` → `update_settings({settings:{retentionDays:7}})` / `run_action("apply_retention")` → `core.rs:824` → `queries.rs:1006-1033`；每轮正常 scan 也会在 `reader.rs:433-439` 应用留存；读取排除边界取 `reader.rs:515-527` 三类水位最大值。显式补导入口 `core.rs:722-767`。记录删除前/后 fact ID、retentionWatermark、重复扫描后值、显式补导结果和源摘要。不要把后台自动留存已删除之后的一次按钮点击，单独当作按钮导致删除的时间证据。

若选择后续 exact 测试，最小实现只是上述动作经 Rig + Core.action + 真实文件 scan，不应镜像 DELETE SQL 当被测程序。现成 CLI 没有设置/动作入口，不能靠两次普通 CLI 调用证明用户动作生效；直接篡改真实账本也不是验证路径。

### C. 原生 CSV 取消与真实保存

生产入口 `src/shared/bridge.ts:23` → `desktop.rs:523-558` → `Core::export_csv` → `queries.rs:1045-1088` → 系统保存框、`fs::write`。前端已有 `contracts.test.ts:67` `CSV cancel is silent, successful filename is explicit, object contract drift never claims success` 及有限 invoke 测试，属于合同，不是实际保存。

同一私有 fixture 先 Cancel：没有新增目标文件，也没有成功提示；再选择新建 output CSV：实际读取 bytes，只有一个 UTF-8 BOM，CSV 能解析，行数/来源线程响应身份/六项计量与独立 oracle 一致，缺失字段留空、记录零保留 0，筛选只导出所选可信响应。隐私模式仅匿名项目，不承诺隐藏来源/线程/响应 ID 或模型。沿用 C18 的公式/换行安全输入；真实 Excel 打开如未执行，仍标“未证明”，不能用解析器替代 Excel。

### D. 正在导入时正常退出与恢复

使用刚好多于一批的小型自建 fixture，等 metrics 确实出现已提交进度且 pending>0 时，从真实菜单正常退出；不要用固定 sleep 猜测“当时正在读”。记录 owned PID+creation-time，`shutdown` 的 scannerFinished/accountFinished/deadlineReached、退出后无自有后代，其他 Codex 身份不变。退出后对私有 SQLite 只读 integrity_check，并保存已提交事实/游标；用同一冻结程序/CLI 重开完成，最终 oracle 一致且重放增量为零，源摘要不变。

准确入口 `desktop.rs:927` quit_app → `:880` begin_shutdown，结束记录 `:919`；Core 取消入口 `core.rs:202` 的 `scan_with_cancel` / reader 的批次事务。现有 C05 提交前故障注入只是事务回滚模拟；C22 是开始前取消；C42 是用户历史取消。三者均不能取代实际进程退出回执，也不需要因新增此回执而重跑它们。

已留存旧 debug 的 `output/native-exit-before-20261015-1622.json` / `output/native-exit-after-20261015-1622.json`、`output/native-care-20261015/quit-observed.json`，只证明各自当时进程的退出，不能重标成 C12。正常退出恢复也不等于任意断电/强杀时刻都已验证。

## 5. G3 仍必须单列的原生通知小场景

实际发送链是 `desktop.rs:1070-1150` process_notifications → `:1609-1648` WinRT，AUMID=`local.tokenmeow.desktop`。Core `core.rs:246-306` 先持久 claim，再写 sent/failed/suppressed；metrics 成功写的是 submitted（`desktop.rs:1146`）。这两种状态都不能证明 Windows 已显示。

最小验证在安装态、私有 source/data 上追加真实 live 结束事件；复用 `scripts/verify-desktop-work-care.mjs:79-88` 的手写 JSONL 结构即可，不在 release 上跑该脚本的 CDP 部分。至少目视实际 Toast 或通知中心内的本地隐私文案，与 inbox 的稳定身份和 result 对应；DND 再追加一条，事实仍在而无主动弹窗，解除后不补弹。同根 unknown→明确只更新原记录且不再提醒。默认 muted 只能证明静音配置/行为，不能自动证明没有通知。

实际拒绝/失败路径只有在已观察到自然权限禁用或通知不可用环境时，核对 failed、错误原因、inbox 保留且重启不补发；未遇到此条件就继续标未证明，不能为了制造失败擅改系统通知设置。多个短事件会按现有队列合并，需以收件箱逐事实与汇总文字共同核对，不能要求一事件必有一弹窗。

## 6. 执行顺序与结论边界

优先 A（现成 CLI + 只读 SQLite，无重建），随后在根的性能窗口结束后按 B/C/D 复用同一个小型私有原生场景；通知安装态单列。G4 三个 case 在后续允许的受影响 exact 或实际面板窗口补证，真实系统时钟跨午夜继续独立记录。

保存新回执时逐一写清执行候选 SHA、输入 oracle、所走生产入口、首次失败与后来结果，不覆盖旧证据。C11 的 P95、安装子范围不能转给 C12；当前构建回执也不能代替 Native/Release 功能通过。本文未发现需要立即修改冻结生产输入才能开展这些最小验证的已确认缺陷，亦不把 G3/G4/G5 或整体 M4 改为已通过。
