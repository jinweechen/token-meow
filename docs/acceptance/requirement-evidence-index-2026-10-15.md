# v0.3 需求与验收证据索引

编制日期：2026-10-15（Asia/Shanghai）；只读核对快照起点为当日 21:16。应用版本 0.1.0，首发范围为 Windows x64、Codex 本机普通 JSONL 及归档移动。本文只新增索引，不更改生产源码、旧回执或旧失败，不运行测试，不将实现、合成样本、浏览器或 debug 局部验证提升为完整 Native/Release 通过。根节点正在执行的新 release 计时、资源和安装场景，必须以其另外落盘的完成回执为准。

依据为《开发方案.md》v0.3 第 3、5—11 节，以及 `docs/contracts/v0.3-implementation.md` 的身份、通知、设置、资源和用户追加关心语契约。下文的源码行号用于定位，不是通过证明。原始方案中的建议目录与实际物理合表差异本身不算缺口，身份、粒度、追溯或必需行为变弱才算。

## 证据等级与版本边界

| 标记 | 能证明什么 | 不能替代什么 |
| --- | --- | --- |
| S：源码核对 | 存在实际调用链、字段或规则；可指出明确静态缺口 | 测试执行、实机行为、发布资格 |
| A：自动契约 | 指定输入和实际断言通过；区分 Core.scan 生产文件路径与直接调用规范化/存储函数 | 未包含场景；真实扩展来源；Windows 窗口行为 |
| B：浏览器生产 UI | 实际 React/资源/渲染生命周期及有限桥接参数 | 真实 Core、系统窗口/托盘/通知、进程 CPU/内存 |
| N：限定原生 | 指定 EXE、fixture、进程及实际嵌入 UI 的观察 | 不同 EXE、物理桌面输入、安装态和完整发布门槛 |
| R：发布门槛 | 同一候选身份、规定输入/硬件/区间和完成结果 | 构建成功、静态安装脚本检查、短 fixture 采样 |

Core 当前声明 **47 项**测试，组成是此前 46 项的组合通过证据，加工作关心语设置 1 项；没有在本轮重新运行一遍 47 项。可追溯执行顺序如下：

| 记录 | 实际范围与结果 | 复用限制 |
| --- | --- | --- |
| 会话工具 session `20453` | 隔离 target 的 `--release --features custom-protocol --offline --lib core::tests`：45 项中 44 通过，C43 首次失败；编译 1m58s，测试 40.47s | 这是原始失败记录，不改为 45/45。其余不变输入的通过保留；原始工具输出在任务会话，未虚构独立日志文件 |
| session `78851` | C43 exact 1/1，通过 3.05s；修正 fixture 缺少可验证 turn_context 后重新验证，未放宽旧累计归属 | 前一次失败的 30 对 0 保留；中间 `63965` 因桥接方法尚未落盘而 E0599，未运行测试 |
| 紧接 `78851` 的 exact health 工具结果 | C45 扩展逐源 unsupported 汇总断言后 1/1，通过 2.02s | 原 44 中同名旧断言由本次受影响补测替代；该次工具无继续运行 session，不捏造句柄 |
| `output/core-f12-directory-result-20261015-182727-036.json` | C41 半行 pending 去重 exact（session `26715`，4.55s）及 C46 目录上限 exact（`11305`，0.91s），各 1/1；留有当时 5 个 Core 文件 SHA | C41 旧版断言不冒充新增半行断言已测。C46 为新增；目录超过 10,000 条在第一笔写入前拒绝，不宣称 100,000 条目录已导入 |
| `output/core-work-care-result-20261015-195851-476.json` | C47 exact，session `49313`，编译 42.33s，测试 2.12s，1/1，66 filtered | 仅新增设置契约；没有再次运行此前 Core/desktop/bridge 用例 |

以上即 42 项保留通过（C01—C40、C42、C44）+ C43/C45/C41 三项补测 + C46 新增 = 46；再加 C47 = 47。此前共享 target 的 E0463 失败没有运行测试，不能计入通过。此前文件调度测试的首次失败也保留，后来修复为有界单来源事务，未扩大原断言窗口来获得通过。

当前设置源码冻结摘要：`core.rs` 为 `FE7F805C2AFAE64338AD337D510EC0A2DDD8B59F418F527DE2E944E7C6D9978E`；`core/tests.rs` 为 `4D37D51DBBE2F38EB44A5938EA2AFA3941A9E7D7918C868DC4B9FB539CAB2F69`。reader/adapter/queries 的旧回执摘要仍可独立比对；关心语只改设置默认和验证，没有改计量/reader。

**旧大样本回放不得冒充双游标新版验收。** `docs/acceptance/ledger-replay-2026-10-15.json` 的 6 份脱敏真实日志、3,004 原生响应、483,884,275 token 与独立 oracle 一致，以及二次回放新增响应为 0，均为历史有效结果。该结果早于 F12 独立历史/实时游标修改，仍证明旧输入的旧运行；不能据此写成新版 reader 已完成同一 3,004 响应真实回放。新版有 C01—C05、C23/C30—C32、C39、C41—C44 的固定 fixture 生产路径证据；新版同一真实脱敏大样本的独立全量对账尚缺完成回执。历史回放计时也不是 UI 延迟、暖启动或资源门槛。

## F01—F12 映射

| 需求与方案位置 | 当前实际源码链 | 已有命名测试 / 回执 | 尚未证明或边界 |
| --- | --- | --- | --- |
| F01 用量概览（59、253—267） | `core/queries.rs:5` 上海日期、`:122` 加载/筛选、`:388` 有效子集聚合、`:426` snapshot；`Overview.tsx`；`shared/format.ts` | C01、C06、C11、C12、C29、C36；前端 `missing values stay distinct from recorded zeros`、`pet small card uses today-only ledger query and panel independently defaults to seven days`；旧 `frontend-final-check-result.json` 的相关查询/选择器不变范围；debug 原生 1200/1 响应，care 1200→1212/1→2 | 响应次数与可信可计量响应次数分开。未有所有日期预设和自定义边界的当前 release 端到端矩阵；上海午夜边界见 G4 |
| F02 缓存（60、257—269） | `adapter.rs:489` 包含关系与异常；`queries.rs:389` 加权分子/分母及覆盖率；概览缓存分区 | C01 的 80%/普通输入 100、C11 缺失/零/异常、C12 有效子集命中率 0.5/输入覆盖 0.25、C25 第二语义手算；旧真实回放只见写入 0 | 非零缓存写入由固定样本证明，不能说真实本机已见非零。没有每一种非法组合/全部缺失组合矩阵；不升级为跨工具效率指标 |
| F03 回合时间（61、284—290） | `adapter.rs:960` 结果及耗时/TTFT、`:1283` 工具单位；`Panels.tsx:213`、`Overview.tsx:474`；`shared/format.ts` | C01 工具 1s+500ms=1500ms、回合 TTFT=500ms；C41 历史重放保留原 startedAt/新终态；前端 `latest turn missing TTFT stays missing instead of borrowing an earlier measurement`、`display time is explicitly Asia/Shanghai and rejects bad timestamps` | 不声明单响应生成速度或首可见文字延迟。当前 Core 无独立上海午夜、空 TTFT、多并行计时的完整断言组（G4）；原生只涵盖指定 fixture |
| F04 项目与任务明细（62、290—323） | `queries.rs:33` path:/mapped: 稳定项目身份、`:122`/`:326` 事实筛选和元数据归因；`core.rs` set_project_mapping；`Panels.tsx` 来源+thread 详情与根任务选择 | C07、C08、C13、C20、C28、C36、C38；`contracts.test.ts` 根 null/self、私有标签稳定项目、同 thread 不同来源；`frontend-final-check-result.json` 来源根选择/项目映射/隐私模型；`bridge.test.ts` sourceId 参数 | C36 是 project+model+provider+root 联合条件，不能当每一维独立或所有维度笛卡尔组合验证。来源实例为可登记/迁移的 Codex，真实 Claude/Gemini 没开放；全局 activity 不受面板过滤，子代理分列 |
| F05 账户概览（63、333—335、401—417） | `account_bridge.rs:212` 白名单账户快照、`:285` state DB 目录分页、`:394` 白名单官方 usage；`desktop.rs:547` 来源绑定/缓存；`OfficialUsageTrend.tsx:13` 官方原日期独立趋势 | 桥接 13 项历史契约，含 `quota_multi_bucket_missing_is_not_zero`、`changed_account_drops_cached_values_even_when_quota_read_fails`、`offline_login_read_preserves_last_collected_data`；新增 clear 1 项；desktop 来源缓存竞态 1 项；`accountUsage.test.ts`；`source-account-final-result.json` 6 项有限 mock；实时桥接 77 日桶、555 目录行（归档 59） | 真实桥接是生产模块 audit example，非所有产品 UI/切换场景。email/plan 不证明账户域，历史仍未知。真实登录切换、断网/超时、重置到点后的产品恢复尚无完整 Native 回执；当前能力报告字段细则见 G1 |
| F06 完成收件箱（64、72、423—425） | `adapter.rs:960` 明确结果映射、`:1150` 根回合唯一收件箱；`core.rs:247` claim/finish 与 mark_read/clear_inbox；`Panels.tsx` Inbox | C01 结果未知/历史不通知、C13 子代理不另提醒、C14 未知→明确保留已读/交付、C33 claim/restart/refinement、C34 DND、C39；`pet.test.ts` 水位/队列；care 原生明确 completed 正常完成 | 当前 Core 断言重心是 unknown/completed；原始 turn_failed/turn_aborted 到 Core→收件箱→实际系统通知/表现的完整链未有当前独立回执（G3）。WinRT 真正显示、合并及安装身份仍单列待验，不把 claim 当系统已送达 |
| F07 聊天入口（65、227、431、524） | `core.rs:338` ValidatedChatTarget，`adapter.rs:366` Codex 受限 UUID 协议；desktop open_thread；`shared/bridge.ts:14` 必传 sourceId/threadId；Panels 保留复制入口 | C40 有效成员、未知来源、任意 URL、带 query 标识、跨源未证身份、未发布适配器拒绝；`bridge.test.ts` 真源码有限 invoke；旧浏览器点击来源参数检查 | 未有 Codex 已开/未开/无效/归档四类真实 Windows 定位完成回执。当前 URI 构造与安全拒绝不等于目标聊天已被正确打开 |
| F08 软预算（66、425—427） | `core.rs:612` daily/project/thread/rootTurn 保存；`queries.rs:810` used/period、`:893` 来源+目标、`:912` 跨线；根任务含明确关联子代理，跨日不重置 | C16 规则版本去重、C38 source+root 与明确子代理跨日计量、C39 真实文件新增跨线 claim 且清 inbox 不补回；`frontend-final-check-result.json` 根/会话来源 payload | C16 名称含周期和档位，但实际只断言同日重复+阈值修订，不证明午夜重置/多档位组合（G4）。项目预算与 worktree 映射有实现，缺独立完整跨线 UI/系统提醒回执；不改变 Codex 执行 |
| F09 工具简表（67、288、467、522） | `adapter.rs:1206` 结构化结果白名单、`:1298` 生命周期；`queries.rs:326` 范围传播；`Panels.tsx` Tools | C01 exit 7/duration 1500；C37 结构化 exit 2/1250ms 且不存 SECRET、伪 stdout 退出码不采信；C36 tools 跟随联合筛选；C41 历史开始不能覆盖新失败终态 | 正常/非零有局部断言；主动取消、MCP isError、并行调用及所有筛选维度缺独立生产路径/Native 完整证据（G3）。工具完成不证明代码测试或业务验收 |
| F10 来源与健康（68、233—237、381—391、484） | `core.rs` add/update/set_primary_source；`queries.rs:70` 逐源 health、snapshot sourceCapabilities/channels；`reader.rs:839` 最近检查/成功/历史 pending；`SourceDetails.tsx:93` 最近检查与无错检查/事件/范围分开显示 | C20、C21、C24、C30—C32、C45、C46；`source-account-final-result.json` 6 项来源严格隔离/null未知/主源 payload/feature.runtime 与 sourceRuntime 分列 | observed range 只是已观测事件范围，`coverageComplete=false`，不是无缺口覆盖声明。压缩格式未解压但统计未覆盖数量；未检查来源使完整 unsupportedFiles=null。逐能力证据字段尚未完全满足方案（G1）。真实多来源故障/主账户热切换目前主要是固定 Core 与 mock UI/缓存竞态 |
| F11 桌宠体验（69、84—145、239—245、547—551） | desktop 窗口/托盘/快捷键/原生事件/退出；`presenter.ts`→`animationController.ts`/`poseController.ts`→`CatGirl.tsx`；`atlasRegistry`/`spriteSheetRegistry`；Core 唯一 settings | 前端 pet/format/popover/atlas/pose/viewRefreshLoop 命名测试；`pet-anime-v4-2026-10-15.json` 12 姿态、13 逻辑帧、30 mount+30 hide/restore、深浅底合成；debug v4 8 次 popover/真实 alpha0；care 原生 9 次+popover 8 次；desktop 6 项单元 | 真正物理桌面点击到下层、连续键盘输入不夺焦点、单实例、托盘/快捷键冲突、负坐标多屏/DPI/拔屏/RDP/睡眠、隐藏但采集继续均不可用浏览器或几何函数代替。完整 Native/Release P0 仍待对应回执 |
| F12 数据管理（70、377—395、469、525） | `queries.rs:1035` CSV安全、`:1045` 仅可信原生响应导出、`:1006` 留存；`core.rs:696` pause/resume/cancel/import、`:769` 清自身表/水位；`reader.rs:11`/`:35` 双游标，同事务；desktop 清 RAM 缓存/代次 | C02—C05、C17、C18、C21—C24、C35、C41—C44；新增 bridge clear 与 desktop cache；`history-control-final-result.json` 6 项 mock action/未知显示；care source-write-audit 确认源文件只多验证器所写 4 行 | C41 真验证旧文件新增尾部+新文件、暂停/恢复与重叠不重计；C42 cancel 重启不复活、显式重导；C43 旧累计独立基线；C44 旧 context/归档/截断。实际 CSV 保存/Excel、留存删旧/水位/补导交互（G5）、真实迁移失败回退（G2）尚无完整回执。匿名仅项目，source/thread/response/model 仍保留，不承诺完全匿名 |

用户追加工作联动/关心语属于 F11 补充：`core.rs:519` 三设置严格验证、`:834` 默认；`workCareController.ts`、`workCareSession.ts` 与 `WorkCareBubble.tsx` 使用新鲜全局 activity/既有提示水位，mixed 交替，不调用模型、不新增系统通知。C47、`workCare.test.ts` 原 6 项加 stale-manual 针对 1 项、`docs/acceptance/work-care-2026-10-15.json` 已汇总 browser 与独立 debug fixture。连续关心是确定性连续观察测试，不能说已实际陪伴真人工作 25 分钟；快照断流、回退时钟、来源变化需重建基线。

## 47 个 Core 命名测试的实际断言索引

以下名称均来自当前 `src-tauri/src/core/tests.rs`。A-file 表示真正执行临时文件→Core.scan→SQLite→snapshot/action；A-unit 表示直接规范化/存储/查询调用，不能说走过文件发现和字节游标。

| ID / 行 | 精确测试名 | 已断言的范围；保留的限制 |
| --- | --- | --- |
| C01 :74 | `fixture_native_and_cumulative_are_not_added_twice` | A-file；1200总量/1响应/80%缓存/普通输入100，累计及嵌套不相加，工具7/1500、unknown、历史不通知、TTFT500 |
| C02 :92 | `repeat_copy_archive_and_restart_preserve_exact_totals` | A-file；复制、归档物理id保留、重开后总量及1 inbox不增 |
| C03 :128 | `byte_cursor_waits_for_split_utf8_complete_line` | A-file；UTF8半字符/半行不提交，补全后120、中文cwd正确 |
| C04 :153 | `truncate_and_replace_same_path_start_new_physical_identity_without_double_usage` | A-file；截断/删除重建，1320不重复且新物理记录 |
| C05 :176 | `before_commit_failure_rolls_back_facts_and_cursor_after_commit_restart_is_idempotent` | A-file；注入提交前失败无facts/ingested_records，重试1200且重启不增；不是实际杀进程 |
| C06 :197 | `conflicting_identity_excludes_whole_fact_and_never_overwrites_original` | A-unit；同域异计量整事实排除可信总量，原100不覆盖，冲突1 |
| C07 :212 | `metadata_enrichment_does_not_change_usage_identity` | A-unit；后补模型不新计量；并非账户切换实测 |
| C08 :225 | `independent_namespaces_same_short_id_are_separate_legitimate_usage` | A-unit；第二独立 namespace 相同短id合法，240、冲突0 |
| C09 :238 | `verified_stream_revision_never_accepts_late_partial_over_final` | A-unit；合成已允许修订1→2 final，迟到partial不覆盖；未穷举同序异载荷 |
| C10 :255 | `codex_native_does_not_allow_metering_revisions` | A-unit；Codex未验证修订时异计量冲突 |
| C11 :268 | `missing_zero_invalid_and_unknown_are_distinct` | A-unit；cacheWrite缺失≠零，cacheRead>input异常，unknown≠missing |
| C12 :285 | `cache_ratio_uses_only_valid_subset_and_reports_input_coverage` | A-unit；有效子集命中率/覆盖率，不把缺缓存字段当未命中 |
| C13 :301 | `child_inherits_parent_history_without_reassigning_parent_usage_or_notifying_child` | A-unit；直接 ingest，拥有者与继承元数据分开、父子总240、子回合不另通知 |
| C14 :332 | `completion_refinement_updates_one_record_and_preserves_read_and_delivery` | A-unit；unknown后补completed只1条，保留read，不重复eligible |
| C15 :363 | `legacy_resets_and_model_switches_create_gaps_not_fake_delta` | A-unit；旧累计100/150/10/40只证明88估算，模型变化不补假差值 |
| C16 :400 | `budget_identity_is_per_rule_version_scope_local_day_and_tier_not_usage_event` | A-unit；同日两usage仅1提醒，阈值规则改版另1；未切换本地日或多个档位 |
| C17 :424 | `clear_watermark_prevents_automatic_refill_but_explicit_history_import_is_possible` | A-file；清理后复制不回灌，显式重导恢复1200 |
| C18 :439 | `csv_blocks_formulas_newlines_and_can_anonymize_project` | CSV helper+真实 Core导出；公式/换行安全、项目匿名；不是桌面保存/Excel打开 |
| C19 :453 | `settings_core_values_win_legacy_migration_but_defaults_do_not_block_missing_keys` | 设置；显式muted优先、旧scale→petScale、缺字段迁移一次 |
| C20 :472 | `source_registration_selection_relocation_and_restart_keep_stable_identities` | 注册；新源不同id、迁移路径重启保持id/主源、可停用 |
| C21 :498 | `rejects_data_source_path_overlap_before_writing_any_database` | 文件路径；双向嵌套/更新拒绝且不先建数据库；未实际创建junction竞态 |
| C22 :516 | `cancellation_does_not_commit_a_partial_record` | A-file；cancel预先为true时不入账，取消撤销后1200；未在中途读/提交瞬间触发取消 |
| C23 :526 | `oversized_half_line_makes_bounded_progress_then_recovers_following_valid_line` | A-file；10MiB半行有界discard进度，补换行后后继有效响应可入账 |
| C24 :558 | `malformed_complete_record_does_not_stop_other_records_or_store_body` | A-file；坏行后有效120，issue message不含private-body；不等价全库隐私扫描 |
| C25 :627 | `second_semantics_fixture_normalizes_exclusive_input_and_never_double_adds_cache_subtypes` | 合成adapter；非缓存input+读+写→1000，普通100/总1200/80%，reasoning未知；不是Claude采集 |
| C26 :649 | `aggregate_quota_and_context_snapshots_cannot_manufacture_response_rows_or_tokens` | A-unit；累计aggregate去重且quota/context不造响应或token；不是OTel delta/reset/重叠算法已发布 |
| C27 :691 | `cross_source_suspicion_excludes_exact_range_until_copy_evidence_resolves_it` | A-unit；疑点事实排除，无证明拒绝合并，有人工证据引用后只计1次；不是自动猜测复制 |
| C28 :713 | `global_activity_is_unfiltered_counts_root_tasks_and_rejects_future_activity` | A-unit；昨日/别model过滤无turn仍保留全局根active1；未来活动降级uncertain |
| C29 :743 | `a_valid_response_identity_with_missing_usage_still_counts_as_recorded_not_trusted_metered` | A-unit；缺output但有效身份recorded1/trusted0/total null |
| C30 :757 | `static_head_files_do_not_starve_older_history_or_a_second_source` | A-file；160静态头文件+第二源在原40批内161/19320；不等价发布多来源吞吐预算 |
| C31 :781 | `file_growth_with_restored_mtime_is_still_collected` | A-file；追加并还原mtime仍240，不只依赖mtime |
| C32 :801 | `locked_source_recovers_after_release_while_another_source_progresses` | Windows文件路径；share_mode(0)锁A不阻B，释放后240；不是睡眠或断网 |
| C33 :828 | `notification_claim_is_atomic_preserves_refinement_and_never_replays_on_restart` | 手工live Context+真实SQLite claim；一次领取、shown、refinement及重启不补发；未调用WinRT |
| C34 :882 | `do_not_disturb_suppresses_live_notification_persistently_instead_of_delaying_it` | 手工live Context；DND持久suppressed，解除不补弹；未覆盖定时DND午夜边界 |
| C35 :914 | `migration_takes_backup_and_keeps_existing_settings_and_source_identity` | 实际v1数据库→v3；backup1、muted/source id保留、user_version3；未放入旧usage或注入迁移失败 |
| C36 :953 | `all_fact_views_honor_project_model_provider_and_root_filters_and_missing_root_usage_is_related` | 直接ingest+snapshot；联合4筛选只剩1会话/turn/tool/inbox，未显式root usage关联正确，同名目录不合并、显式映射可合并，隐私模型值不被改坏 |
| C37 :1040 | `structured_tool_result_metadata_is_retained_but_body_and_spoofed_stdout_are_not` | 直接ingest；结构化工具exit/duration可取、SECRET不存，伪stdout exit不取 |
| C38 :1089 | `root_task_budget_includes_explicit_children_across_days_and_never_other_source_same_ids` | A-unit；source+root自身与子代理跨日240，不含另一source同id；会话预算仍按本地日 |
| C39 :1138 | `live_budget_crossing_claims_once_and_cleared_inbox_does_not_refill_same_threshold` | A-file；历史基线不通知、真实尾部跨线claim1、清收件箱不回填同阈值 |
| C40 :1165 | `chat_target_requires_exact_source_membership_registered_adapter_and_plain_uuid` | 目标构造/拒绝；准确source、成员、身份域、UUID、注册adapter；未执行外部链接 |
| C41 :1289 | `paused_history_keeps_old_cursor_but_collects_new_files_and_old_file_tail_without_recount` | A-file；约6.5MiB历史未追平时pause，旧file半行+尾部及新file实时，pending单file计1；resume1080/4且新终态不被老事件覆盖 |
| C42 :1387 | `cancelled_history_survives_restart_and_requires_explicit_reimport_while_live_remains_active` | A-file；cancel/重启保留，resume拒绝，实时尾部继续，显式重导1080/4 |
| C43 :1414 | `independent_live_legacy_cursor_needs_its_own_baseline_then_full_history_fills_only_proven_gap` | A-file；实时旧累计首基线不跨未读间隙，下一增量30，历史追平80、再扫仍80；fixture有明确turn_context |
| C44 :1449 | `v3_context_adds_live_cursor_without_resetting_history_and_both_survive_archive_and_truncate` | A-file；移除旧context live_cursor后兼容新增，pause/归档/截断实时4响应；不是完整DDL失败恢复 |
| C45 :1497 | `per_source_health_is_unfiltered_has_real_success_watermark_and_preserves_unsupported_capability` | A-file；范围不受过滤，未知源null，能力runtime不被源runtime覆写，unsupported待未知后求和1 |
| C46 :1544 | `oversized_directory_is_rejected_before_any_partial_metadata_write` | action/SQLite；10001条拒绝，旧count1/title不改，报安全上限/未声明覆盖；不证明10000条完整高规模性能 |
| C47 :1580 | `work_care_settings_defaults_validation_atomicity_and_restart_round_trip` | 唯一preferences；旧记录默认、disabled/重启、8种非法整次不写、合法三语气/三间隔、enabled重启 |

## M0—M4 与证据入口

| 阶段（方案行） | 已有证据 | 完整阶段仍欠的证明 |
| --- | --- | --- |
| M0（496） | `M0-环境与发布门槛.md` 固定Windows/i5-12400/12逻辑核/约32GiB、版本和门槛；`tests/fixtures/codex-native.jsonl`、`multi-semantics.json`；身份/语义标准类型 `adapter.rs:7—264`；桥接实时只读账户及目录555、原Codex保留/config-auth摘要不变；部分窗口透明与debug行为 | M0的聊天实际定位、完整Windows穿透恢复等不能从schema/源码推定已过。各来源能力证据细则G1；合成第二语义不等于Claude本机接入 |
| M1（497） | 当前Core组合47项中除care外46项，恢复/双游标固定样本生产路径；WAL+FULL、事务与版本备份 `core.rs:92`、reader提交；旧3,004响应oracle回放作为历史证据 | 新版双游标reader同一大样本对账；完整迁移事实保持及失败恢复G2；上海跨日/模型无回合/来源重叠边界的未覆盖子场景，不因文件或表存在认为通过 |
| M2（498） | 真实生产PNG哈希/解码/URL所有权测试；v4 30+30浏览器循环、静态回退/过期加载；presenter未知/失败/多任务；桌面几何6测试与限定debug原生alpha/popover | 用户输入不抢焦点、物理穿透、托盘/冲突、多屏/DPI/RDP/睡眠等完整P0原生；Windows资源和重复生命周期进程增长，浏览器资源计数不能替代 |
| M3（499） | 账户白名单mock+实连audit；收件箱/预算claim/工具/安全CSV/历史双游标；来源缓存及清理代次；care生产collector→Core→嵌入UI自造fixture链 | 系统通知实际显示/失败/安装身份，聊天定位四场景，产品实机认证/离线恢复，真实文件导出、留存与清理账户缓存完整UI闭环；部分结果分支缺独立测试（G3/G5） |
| M4（500） | NSIS配置/模板静态核对，已构建候选EXE/安装包哈希；21:24补充的 `output/release-care-legal-f8d64-artifacts-20261015.json` 14项静态检查。代码/素材清单分离、来源/生成依据/摘要、未签名限制。`docs/release/0.1.0/` 三份用户文档在编制 | 最终候选hash对应的完整安装/升级/卸载、WebView2缺失、R门槛与持续使用；当前进行中的native运行不预先计为完成。旧候选和旧模板hash不能替代本轮legal更新后的最终包 |

## 显式发布门槛与未完成的原生场景

门槛来自方案 475—490、M0 冻结记录；本文不放宽。现有脚本或其自检只证明测量机制，不证明产品通过。

| 门槛 | 固定口径 / 工具 | 本索引已有证据与尚需回执 |
| --- | --- | --- |
| 增量可见 P95 ≤5s | 单一正常本机来源，历史导入结束，至少20完整事件；完整行可读后→实际界面更新。`prepare-incremental-fixture.ps1`、`visible-increment-observer.mjs` | helper从写前计时至实际accessibility返回，包含flush/观察费用，是保守上界；20样本nearest-rank第19项。当前helper只给 `threshold-met-awaiting-independent-process-and-visibility-binding`，必须由原生owner独立绑定运行image/PID/creation↔所选窗口及实际可见性，不能把timing-only当发布passed。未见已完成当前release20事件回执 |
| release暖启动 ≤3s | 已建库，不等历史/账户；真实进程创建→本地界面可交互契约。`observe-runtime-preconditions.py` | 创建FILETIME到首个外部ready观察含调度/落盘费用；缺ready、起点不覆盖或时钟偏差>50ms不qualified。两RAF/bindings信号不额外声称像素解码或所有物理输入均完成。debug冷启动/后端查询延迟不替代本项；本轮实际结果由root另记 |
| 10分钟CPU ≤1% | 12逻辑核归一化，完整≥600s；无导入、主面板关闭、宠物静态/低频、普通桌面状态；包含WebView2及所有自有子进程 | `resource-job.cs` 创建挂起→入自有Job→恢复，累计CPU含已退出子进程；`resource-metrics.ps1`区间差/单调检查。受控短子进程曾在两次1s快照之间消失仍被Job累计捕获，只证明observer。PID轮询模式不能通过全进程CPU覆盖门槛。本索引不将fixture-only或运行中采样计为产品通过 |
| 常驻私有内存 ≤350MiB | 同一完整600s条件；均值和采样最大值都记录，当前gate两者均≤350；全部自有进程 | 一秒采样、梯形时间加权均值、采样间隙>2.5s取消资格；不是连续绝对峰值。Job lifetime peak commit单独报告，不拿commit替换private。浏览器heap/URL/bitmap计数不是进程私有内存 |
| 首次近30天导入峰值 ≤600MiB | 记录文件数/字节数、导入开始/完成、历史边界；含WebView2及自有进程 | `Scenario import`即使满足内存也只输出 `memory-only-passed-import-completion-not-proven`。6份旧回放的26.862s不是新reader导入内存/30天覆盖完成证明。需本轮真实导入完成及资源区间匹配回执 |
| 正确性/恢复 | 固定oracle、复制/fork/子代理/嵌套、冲突、半行/截断/归档/锁与故障 | C表给出实际自动范围；旧3,004回放版本过期边界明确。真实睡眠/中断升级/实际进程崩溃另证 |
| Windows兼容与P0 | 单实例、100/150/200%DPI、多屏负坐标/拔屏/RDP/睡眠、置顶/穿透/找回、启动自动变化不夺焦点 | desktop几何单元不等于这些原生场景。debug自有窗口OS焦点转移与CDP输入有局部证据，物理桌面点击/连续输入仍独立，不能以之写全F11通过 |
| 安装/升级/卸载 | 干净安装、已有数据升级、迁移失败保留、WebView2缺失在线/离线、卸载保留/显式清理、下次登录自启、安装身份通知 | NSIS静态分支/可编译hook不等于执行成功；x86 NSIS wrapper包装x64 payload是可解释结构，未签名如实列限制。根正在实测，本文不预写结果 |
| 分发与长期生命周期 | 最终包/EXE哈希、版本、使用/口径/限制、代码与素材独立清单；重复资源加载销毁和连续使用 | v4及care browser循环、debug退出回收有效但范围有限；最终法律资源更新后的产物须重新绑定清单/哈希。P1/P2/真实扩展适配器保持范围外 |

observer自检的 retained 短进程成功样本在 `output/performance/observer-test-776f2f4fe49d4565ad0f99c753c7719b/`；`test-summary-20261015-171345-286.json` 的24检查含复用未变的短进程样本和新增协议/防护，不是一次新的全产品运行。之前.NET及原生重试未满足“一秒内子进程退出”前提的失败目录保留，不能选性删除或改写成通过。

## 确定的细则缺口与覆盖矛盾

这些条目分清实现差距和测试缺证，不断言未跑场景必然失败，也不擅自扩展冻结源码。

| ID | 类型 / 精确依据 | 触发与影响 | 补齐建议 |
| --- | --- | --- | --- |
| G1 | **实现细则差距**：方案233要求逐能力的版本/通道/粒度/前置条件/证据/最近验证时间。`adapter.rs:372—373` 只有usage比较完整，其余多为support/runtime；没有逐项verifiedAt，额度能力也未列入该sourceCapabilities报告。`SourceDetails.tsx:185`只呈现已有字段 | 当前F10可显示来源/能力运行态，却不能把所有能力行称为已具完整可追溯报告。C45只断言runtime隔离/health，未证明字段完整性 | 将本机能力验证收据与明确时间/通道/粒度逐项关联，未知字段显式未知，账户额度按实际来源声明；补真实snapshot合同断言。发布前由owner决定是否作为未闭合细则处理 |
| G2 | **迁移缺证**：C35 `tests.rs:914—949` 初始只放source/settings，断言backup/user_version；方案379、392、525、538还要求原有效总量保持与升级中断/失败恢复 | 不能凭这一项写“旧用量迁移、升级中断回退全部通过”。`core.rs:106—139`有备份/事务，但源码不是恢复验证 | 以后针对旧有效usage/缺字段样本及迁移失败/中断做oracle与重放，检查原DB/备份可恢复；本索引不重跑/不修改 |
| G3 | **结果/通知/工具链缺证**：Core已有 `adapter.rs:960—969` failed/interrupted映射，但47项没有独立原始turn_failed/turn_aborted完整断言组；C37也未覆盖全部MCP错误/主动取消/并行。方案521—524要求完整场景 | presenter输入失败能正确显示，不证明原始日志经collector/收件箱/系统通知必定正确；claim/shown测试不证明WinRT/安装AUMID实际显示 | 追加对应输入的独立生产链及Native收据；保留工具结果与业务验收边界。当前不称这些分支已验收 |
| G4 | **测试名称强于断言范围**：C16实际无跨午夜/多档位；C36是联合4条件；C28有过滤与未来时间但不是所有活动转换。方案100、427、518/521/537还要求上海跨日/计时缺失/完整范围 | 47项数量不能推出所有日期×来源×提供商×模型×项目×根任务×代理组合或每个预算周期边界已通过 | 精确列出现有断言（见C表），以后只补没有覆盖的边界；不要要求无意义全组合或重跑不变成功项 |
| G5 | **留存和桌面数据动作缺证**：`queries.rs:1006—1032`有留存删除和水位，当前47项无直接apply_retention断言；C17验证clear/import，C18验证CSV内容而非真正保存。C22是开始前取消，非中途取消 | 不能把这些测试概括为所有留存/导出/中途退出/清缓存UI完整闭环。源只读另有路径拒绝与source-write-audit，不能替代留存事实恢复 | 以后补旧/新事实留存水位、重新复制/显式补导、真实保存与源摘要、退出边界；保留当前数据动作局部通过 |
| G6 | **证据版本差异**：3,004真实日志回放先于F12双游标；SVG/BBO阶段浏览器证明的无关选择器合同仍可保留，但不能用来验v4 PNG；debug N证据不能替换release R | 若直接合并成“本次全量账本/全桌面/全部发布通过”，会产生范围与版本矛盾 | 按本文等级、文件SHA和输入范围复用。新reader真实大样本对账与最终release原生门槛分别留新回执，不回写旧通过 |

## 可检索回执清单

| 路径 | 范围 |
| --- | --- |
| `docs/acceptance/M0-环境与发布门槛.md` | 初始字段/能力/环境/硬件/发布目标及只读桥接补充 |
| `docs/acceptance/ledger-replay-2026-10-15.json` | 旧reader 6日志/3,004响应的精确oracle与幂等历史运行 |
| `docs/acceptance/account-bridge-contract-2026-10-15.json` | 13项mock生产桥接契约，首次失败日志保留 |
| `docs/acceptance/live-account-bridge-2026-10-15.json` | 真实audit账户/77日桶/555目录及原进程/config/auth不变 |
| `docs/acceptance/account-bridge-clear-2026-10-15.json` | 新增bridge清缓存1项，保留source和cancel Arc |
| `output/desktop-focused-tests-20261015.json` | desktop6项：来源缓存/迟到回复、3类几何恢复、小屏限制、通知安全文字；非Native |
| `output/core-f12-directory-result-20261015-182727-036.json` | C41/C46两exact和当时源码摘要 |
| `output/core-work-care-result-20261015-195851-476.json` | C47唯一设置exact与当前core/tests摘要 |
| `output/playwright/frontend-final-check-result.json` | 旧SVG阶段8项UI；只复用不变查询/来源payload/隐私/布局，不能验新PNG |
| `output/playwright/source-account-final-result.json` | 官方原日期趋势、主源及逐源health/capabilities有限mock UI |
| `output/playwright/history-control-final-result.json` | pause/resume/cancel/import及unknown显示的有限mock UI |
| `docs/acceptance/pet-anime-v4-2026-10-15.json` | 当前两张原PNG、12姿态/13帧、browser与限定debug汇总；遮挡手指不算五指认证 |
| `output/playwright/atlas-v4-states-result.json` | 真实PNG解码与语义状态；合成事实/测试时钟，非Native状态采集 |
| `output/playwright/atlas-v4-lifecycle-result.json` | 真实解码/URL/选定监听器/调度30+30循环和故障/过期加载回收；非Windows资源 |
| `output/playwright/atlas-v4-composited-result.json` | 原PNG深浅底合成，源像素未改 |
| `docs/acceptance/work-care-2026-10-15.json` | care设置/前端/真实自造JSONL→debug UI9项、popover8项及正常退出汇总 |
| `output/native-care-20261015/source-write-audit.json` | 源只增加验证器明确写入4行，产品未额外改源 |
| `output/native-care-20261015/quit-observed.json` | 指定debug进程正常退出；不证明所有升级/安装退出情形 |
| `output/release-care-nsis313-artifacts-20261015.json` | 留存此前候选安装包/EXE/模板hash；非最新legal包，不当最终安装执行证据 |
| `output/build-release-care-legal-f8d64-20261015.json`、`output/release-care-legal-f8d64-artifacts-20261015.json` | 最新legal-only rebundle与14项静态检查：应用 `C11DD01F757FFD0E6F07FB6926A334E48D7A19BFB3C083CCDEC3E05441AA05C2` 不变；setup `884964DC41015CB9A4912ACC22E49854076B6A93064E0306EF0A82E19E28EAAD`；未签名、未据静态结果声明安装成功 |
| `output/distribution-licenses/manifest.json`、`assets/pets/nekomi/provenance.json` | 当前分发清单及素材生成依据，须与最终候选hash对应 |

P1 可选皮肤导入/像素自动命中/受限移动、P2 Live2D/VRM、真实 Claude/Gemini/OTel/Hooks 接入和各研究指标均不在 V1.0 既定发布范围。合成类型隔离可以证明通用层契约，不能改变这些接入状态。本文不以研究项待定替代核心发布门槛。
