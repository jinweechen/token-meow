# Token 喵

Windows 本地 AI 用量桌宠。首版接入本机 Codex，默认角色为本项目设计、AI辅助生成的日式 Q 版猫娘“小柚”。用量统计、规则和角色动画在本地运行，不需要额外调用模型。

猫娘保留猫耳和猫尾，使用正常的人类手脚。2026-10-18主源码默认使用[v11新素材包](docs/acceptance/pet-animation-v11-integration-2026-10-18.json)：52张1024×1536透明PNG、12个动作与8相位原地侧走演示，包含新过渡和保留的v6动作主体。原图、提示词与来源保存在 `assets/pets/nekomi/walk-v10-complete/` 和 `animation-v6/`；仅打包入选整角色PNG，按当前片段串行加载，固定画布和共享锚点。持续工作/提醒只循环主体，空闲动作和提示按完整时长播放；减少动画显示静止待机，免打扰显示静止坐睡，隐藏与来源切换取消旧资源。分层素材保留作后续制作，不进入运行包。

v11已完成73项定向测试、类型检查、前端构建、分发来源生成及浏览器组件/最终网页检查。侧走仅原地右向展示，Native游走、安装包、性能及发布验收未纳入本次范围；现有C33等封存候选保持历史归属。

当前开发基线是[开发方案 v0.3](开发方案.md)。本目录仍在开发；是否可发布以[验收追踪](docs/acceptance/v0.3-验收追踪.md)和实际证据为准。

源码仓库包含代码、角色素材、许可补充文件和构建必需的8份素材来源记录；`output/`中的其余构建包、数据库、运行日志及本机验收原始证据保留在开发机，不随Git提交。验收摘要中的本机证据路径用于追溯，并非仓库内可下载文件。`.gitattributes`保留文件原始字节，避免换行转换破坏素材来源摘要。

2026-10-18 追加的本地扩展开发包含 PNG 目录包导入/预览/显式选择与回退、保守透明区域命中、15 秒临时交互。[开发记录](docs/acceptance/p1-local-extensions-development-2026-10-18.json)中的当前前端225项、真实模块测试18项、前端构建和桌面源码类型检查通过；接口和开发开关见[本地 PNG 皮肤目录包 v1](docs/contracts/local-png-skin-v1.md)。这三项普通启动均未开放，等待新版 Windows App 的原生与资源验收；前端、模拟 IPC 或类型检查不表示发布合格。

2026-10-18 V1.0续验收：前端全量220项通过；[C34 App构建](docs/acceptance/c34-v11-v1-build-failure-2026-10-18.json)因全机提交内存耗尽失败，原记录保留。另行构建的[当前headless CLI与受限Core核对](docs/acceptance/c34-headless-core-scoped-2026-10-18.json)通过：实际首次导入和重放均自然exit0/Job0，手工样本全期3050 token／8响应，独立SQLite分期为今日2950／7、昨日100／1；计量、工具、历史收件箱和隐私标记检查通过，重放事实保持稳定。独立复核已完成；该结果不代替日期查询界面、新App／安装器、原生交互、性能和M4验收。

后续以C33旧包补做了[不变数据组件的原生界面检查](docs/acceptance/c33-core-ui-native-scoped-2026-10-18.json)：今日2950／7、近7天与30天3050／8、自定义昨日100／1，以及细分、会话、工具、收件箱、健康与昨日CSV；关闭后的数据库与CSV只读复核11组通过。旧包仅用于该组件范围，不能代替v11动画、当前性能或安装验收。两次启动失败均保留；隔离用户目录补齐后才成功运行。实际菜单退出为root0／Job0，但子进程身份12／18存在缺口，完整生命周期仍未合格。另修复了[显式数据目录仍提前读取默认目录的问题](docs/acceptance/data-dir-lazy-fallback-2026-10-18.json)，实际原表达式4通过／2失败、修复表达式6通过。此修复尚未进入新App，后续构建需要重新冻结源码。

当前完整三产物是[C33候选](docs/acceptance/c33-v6-panels-headless-build-2026-10-17.json)，[独立后核R1](docs/acceptance/c33-v6-panels-headless-post-audit-2026-10-17.json)通过。新冻结源码纳入Panels两项修复及独立headless CLI入口，保留v6的38帧/12动作；809项源码含本轮生成的526项法律资源，42项dist前后稳定，14项静态检查与588项分发校验通过。产物路径、大小与SHA见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。

六阶段最终均root0/Job0且守卫关闭；App和CLI按预声明清理自有编译器残留，非自然整Job结束，其余阶段自然完成。封存当时App、安装器及生产CLI均未运行或安装；本轮后续C33原生局部检查见上文，安装器仍未执行。真实账户/桥接和模型未调用。构建不扩大旧37/40/R3、Panels9组/33观测或headless分次8语义组/21Core的原范围；完整Native、安装、性能、合格步态与M4仍未通过，不据此证明内存改善。C32的CLI失败和C31R1历史三产物继续保留。

主源码已完成[离线CLI特性接入与受限验证](docs/acceptance/headless-cli-feature-integration-2026-10-17.json)：正常App默认使用desktop，custom-protocol仍包含desktop；离线ledger_audit使用独立headless-audit并关闭默认特性。六项GUI依赖改为可选，Core、账户桥接、通知门、时钟、完整例程及迁移的39项原输入保持原字节，Cargo.lock未改。八组语义检查分次完成，另有21个临时纯Core用例通过；原37项中的16个desktop用例未在本轮运行，不计入通过，旧owner超时、诊断解析失败及跨通道反例均保留。

新的headless CLI已编译归档，采用实际CGU16/jobs1、O3/thin和strip=debuginfo；这不证明内存改善，也没有运行生产CLI、App、安装器或真实账户/模型。该接入回执记录当时的独立CLI验证；三文件与下述Panels修复随后已纳入C33。C32仍是历史部分构建，C31R1三产物保持原样。

历史[C32小卡候选部分构建](docs/acceptance/c32-v6-pet-card-shortcuts-partial-build-2026-10-17.json)已归档App与安装包，纳入v6及小卡快捷入口；配套CLI因LLVM内存不足失败，未执行最终封存、14项静态检查或完整分发校验，因此不是完整三产物候选。App阶段自然退出0、Job归零，809项源码、34项控制与42项dist守卫已关闭。产品路径和校验值见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。C32冻结产品仍保留首次单边设置定时免打扰与会话明细旧统计两项缺陷；主源码的后续修复及受限检查见下文，未回写本包。

主源码已应用[定时免打扰与会话明细修复](docs/acceptance/panel-dnd-session-refresh-2026-10-17.json)：时间两端成对保存、保存中均不可编辑；会话明细按来源＋会话身份读取最新快照，记录移除后关闭且不自动重开，并保留项目映射草稿。旧行为已实际复现，修复后9组真实组件配合有限合成host的检查、33次断言观测及受保护输入的类型检查通过。33次观测包含诊断，不是33个场景；这不证明App.save、Core持久化/调度或Native端到端。修复已纳入C33，C32与C31R1均不含该修复；该组件回执、既有37/40代码及R3小卡证据仍保留原输入范围。

历史[C31R1组合候选](docs/acceptance/c31r1-cli-recovery-composite-build-2026-10-17.json)：复用C31已归档App和NSIS，新构建配套CLI，独立后核见[审查记录](docs/acceptance/c31r1-cli-recovery-post-audit-2026-10-17.json)。恢复副本的804项源码、42项dist与原C31逐字节等价，48项App归档一致，14项静态检查和575项分发校验通过；这不是六阶段全新重跑。三产物路径、大小与完整SHA见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。

新CLI采用根包CGU16、jobs1，保留O3/thin LTO；CLI与seal分别用时231.1566266秒、5.9023174秒，均自然结束、root0/Job0，守卫全部关闭。沿用App仍保留其原root0后清理自有残留、非自然整Job结束的事实；[原C31 CLI内存分配失败](docs/acceptance/c31-v6-functional-reliability-partial-build-2026-10-17.json)没有被改写。实际根包编译参数另观察到`strip=symbols → debuginfo`，不能称仅CGU改变，也不能因本次成功证明单参数降低内存。所有C31R1产品均未执行或安装，Native、性能与完整M4继续后置。

C31R1包含C31冻结时的[工具/会话/查询修复](docs/acceptance/v1-functional-reliability-2026-10-17.json)与[预算实时许可修复](docs/acceptance/f08-budget-live-permits-2026-10-17.json)；预算证据仍为保留68项通过并补4项的组合，不能称一次全69项重跑。后续统计小卡的“查看今日未读”和“返回最近聊天”已完成[代码与受限浏览器检查](docs/acceptance/pet-card-shortcuts-2026-10-17.json)：后端37项、前端40项及12组真实React界面配合合成IPC的交互分别通过，R3夹具另经双页40次并发写入和负例检查；R1/R2失败与旧代码回执原样保留。新小卡已进入C32部分构建的冻结源码，未纳入C31R1；C32的App与安装包已归档，CLI失败且未封存，该时点完整三产物仍为C31R1；当前候选见上方C33。这些检查不代表Native、系统剪贴板、真实聊天定位、安装、性能或完整M4通过；真实走动仍缺合格步态。

v6源码接入的51项定向测试、类型检查和前端构建通过，12动作的浏览器展示、透明深浅底、暂停恢复及来源切换取消已有受限检查。[接入记录](docs/acceptance/pet-animation-v6-integration-2026-10-17.json)说明各次验证范围；[前端目录](output/build-candidates/animation-v6-web-20261017/dist/)仅为该阶段网页构建，Windows当前候选见上方C33，原生交互、安装和性能验收继续后置。

C23基础功能已接回主源码：连续观察2分钟空闲后的原地动作、已登记应用的陪伴提示，以及最多16个.exe的快捷启动。使用入口见[候选版使用说明](docs/release/0.1.0/使用说明.md)。主目录已移除被高清版替代的旧图集与加载代码。已封存C23候选仍固定v4并按用户要求保留；高清接入的检查、构建和实机验收单独记录，实机测试暂缓。

[C24-HD隔离候选](output/build-candidates/c24-hd-source-save-20261016/)已生成App、安装包和配套CLI；16项前端测试、12姿态的组合浏览器检查及21图人工复核已完成，具体范围见[高清接入记录](docs/acceptance/pet-hd-v1-integration-2026-10-16.json)。本包包含下述来源保存修复，未纳入并行用量可靠性更新；主目录改动全部保留。新产物尚未启动或安装，实机、安装和性能验收继续后置。

主源码后续的来源配置回滚、CSV保存和桥接准入修复已有[52项定向验证](docs/acceptance/c24-source-save-targeted-2026-10-16.json)通过；这些修复未进入已封存C23候选。

根据 CC Switch 源码与 Issues 交叉核查，主源码进一步补强了目录发现后的读取预算、部分目录失败隔离、采集问题报告、整数汇总保护和界面刷新合并。账户快照时间缺失、异常或晚于当前时钟时标为过期，保留历史比例并分别显示采集时间与最近尝试。实现约束见[用量可靠性补充](docs/contracts/v0.3-implementation.md#13-用量采集汇总与刷新可靠性c25)，验证范围见[C25定向回执](docs/acceptance/c25-usage-reliability-targeted-2026-10-16.json)；不改写封存候选或既有发布验收。

## 使用方式

主源码已加入用量统计面板：指标分项、小时/日趋势、分页响应、供应商和模型汇总，以及带历史版本的估算定价。在“用量概览 → 定价 → 选择模型”保存models.dev到本地模型的映射后，可启用每6小时自动同步或点击立即同步；默认保护手动价格，缺价不会填成0。同步价从同步时点起生效，历史估算可另加较早生效版本。实现与验证范围见[C26说明](docs/contracts/v0.3-implementation.md#14-用量统计面板与参考价格同步c26)及[定向回执](docs/acceptance/c26-usage-dashboard-targeted-2026-10-16.json)。相关源码已纳入C30；C26回执仍只证明该轮定向范围，实机与发布资格见当前候选记录。

启动后，猫娘悬浮窗提供统计小卡，主面板提供用量、项目与会话、代理、工具、完成收件箱、来源健康和设置。角色支持拖动、缩放、置顶和整体鼠标穿透；通过托盘或 `Ctrl+Alt+M` 找回并恢复交互。透明空白区域的逐像素穿透尚不属于首版承诺。

- 默认静音、开机自启关闭。免打扰期间继续记账并保留收件箱，恢复时不补播短提示。
- 减少动画使用静态姿态，文字、图标和角标继续表达事实。
- 工作联动会根据已确认的回合状态显示本地关心语；温柔与卖萌风格交替，可在设置中关闭、换语气或选择25/45/60分钟的持续忙碌关心间隔。摸摸小柚也会回应，免打扰或隐藏后不补播旧话。
- 正常关闭主面板后应用可驻留；从菜单选择“退出应用”停止采集并回收自有账户桥接进程。
- 日期按 Asia/Shanghai 聚合，底层保存 UTC。首页可选今日、近7天、近30天和自定义日期。
- 项目映射可以将多个 worktree 目录归为同一项目；原目录仍作为来源元数据保留。
- 来源目录默认尊重 `CODEX_HOME`，未设置时使用用户目录下 `.codex`。增加来源和修改路径通过应用内受限来源配置完成，不改 Codex 配置。
- 历史暂停/取消保留已采集事实，实时尾读继续；取消后需要显式补导。会话目录单次超过10,000条会明确拒绝并保留旧快照，不声明完整覆盖。

隐私模式隐藏项目、标题和路径；CSV 导出进行公式与换行处理。清理仅影响宠物自己的账本和提醒，不删除 Codex 源日志；清理水位防止立即回灌，需要时显式历史补导。

## 统计口径

“已记录 token”代表已采集、语义有效且无未解决重复疑点的本机记录，不等同账户账单或全部活动。

- 总量为输入加输出；缓存读取/写入和推理是细分，不能再次累加到总量。
- 缓存命中率按有效记录的缓存读总量除以输入总量，并显示覆盖率。
- 来源未提供、语义未知和异常均不是零。缺少依据的派生值不展示。
- 回合首 token 耗时是 Codex 明确记录的指标，不等同首可见文字延迟或生成速度。
- 日志回合结束没有明确结果时显示“已结束，结果未知”。工具正常退出不代表测试或业务验收通过。
- 账户额度和官方日趋势独立展示，不与本机账本相加，也不把百分比换算成剩余 token。

详细契约见[统计与身份方案](开发方案.md)、[实施契约](docs/contracts/v0.3-implementation.md)与应用内来源能力。

## 本地开发

环境基线：Windows x64、Node 24、pnpm 11、Rust 1.96、Visual Studio C++ Build Tools 和 WebView2。精确依赖由 `pnpm-lock.yaml`、`src-tauri/Cargo.lock` 锁定。

```powershell
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri dev
```

浏览器预览只用于页面与交互检查，明确标记演示/未连接，不作为真实采集或 Windows 桌面验收。

干净克隆在构建 Windows App 或安装包前还需生成分发许可目录。先下载锁定的 Cargo 依赖，并备齐 `%LOCALAPPDATA%\tauri\NSIS` 下的 NSIS 3.13（含 `COPYING`）与 `nsis_tauri_utils` 0.5.3 插件；`scripts/distribution-manifest.mjs`会核对这些本地工具的版本和摘要。生成的 `output/distribution-licenses/` 是 Tauri 资源输入，不纳入 Git；8份已跟踪的素材来源记录必须保持原始字节。

```powershell
pnpm dev
pnpm build
cargo fetch --manifest-path src-tauri/Cargo.toml --locked
node scripts/distribution-manifest.mjs
pnpm tauri build
```

NSIS 安装包位于实际 Cargo target 目录的 `release/bundle/nsis/`，使用 `--target` 时还包含目标架构子目录；可用 `cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1` 核对 `target_directory`。本机已设置全局 `CARGO_TARGET_DIR`，发布构建使用本项目独立目录，避免覆盖其他项目的打包中间物。未签名包如实标记分发限制；生成安装包不代表发布门槛通过。

开发与验收可以用绝对路径环境变量 `TOKEN_MEOW_DATA_DIR` 指定独立的宠物数据目录；它不改变 Codex 源目录。请勿把宠物数据目录设置到 Codex 源目录中。

离线账本回放可使用 `ledger_audit` example，只读账户与分页目录可使用 `account_bridge_audit` example。后者只保留脱敏统计，不输出邮件、会话标题或路径，不发起模型推理、登录或任务控制。示例的证据文件应写入本项目 `output/`；原生窗口与发布验收另行执行。

从当前源码构建离线账本工具使用以下命令；离线环境可另加`--offline`。构建不等于执行回放或完成发布验收。

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --example ledger_audit --release --no-default-features --features headless-audit --jobs 1 --config profile.release.package.token-meow.codegen-units=16 --locked
```

## 边界与后续

首版不控制 Codex 执行，不审批、不切模型、不压缩会话、不修改登录或源配置，不安装 Hooks/遥测桥接，不收集全局输入内容。Claude Code/Gemini 仅为后续适配器；语音视觉聊天、养成、任意代码 MOD、用户皮肤导入、Live2D/VRM 均有独立开放条件。

本项目没有打包调研项目的角色或完整运行时。原创角色与代码依赖分别记录来源、许可和摘要；参考资料见[开源桌宠调研](docs/research/开源桌宠调研-2026-10-15.md)。
