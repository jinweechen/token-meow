# Token 喵

Windows 本地 AI 用量桌宠。首版接入本机 Codex，默认角色为本项目设计、AI辅助生成的日式 Q 版猫娘“小柚”。用量统计、规则和角色动画在本地运行，不需要额外调用模型。

2026-10-19新增[独立自绘安装器](installer/README.md)：奶油白与鼠尾草绿界面展示用量账本、收件箱提醒和桌宠功能，提供安装位置、快捷方式、真实阶段进度、重试及完成后打开。普通安装向导由自绘窗口替代，底层引擎仅静默运行；普通卸载界面保持原状。

当前推荐下载[C41自绘安装包](output/build-candidates/c41-current-v1-20261019/release/TokenMeow_0.1.0_x64-custom-setup.exe)，128,791,040字节，SHA-256 `7CB3F71A16D37E756026D4B9132485B8BA5F33C9CDA9D3E0A1F5FF079E035190`。[C41重封记录](docs/acceptance/c41-webview-execwait-repackage-2026-10-19.json)记录两处WebView2可执行路径引用及启动错误门修复，8个隔离分支实际用例与20项静态检查通过；引擎重封与WPF封装两Job均自然root0／Job0、0终止。App仍为C40实际NSIS payload `6EBA…C15C`原字节，本轮没有Rust／App或headless CLI重编。安装包未签名，界面视觉、真实安装、WebView2下载、升级、卸载、Native及性能尚未验收，整个V1仍未完成。

历史[C40完整源码构建](docs/acceptance/c40-full-source-custom-installer-2026-10-19.json)保留为当前App的来源；该轮旧引擎不含上述WebView2修复。C40的18项静态检查、外层root0／Job0及按预声明清理自有后代、非自然整Job结束保持原范围。后续watcher窄配置读取修复尚未进入当前App，不计入其运行结果或性能收益。

C40已包含[概览采集覆盖提示](docs/acceptance/usage-collection-coverage-2026-10-19.json)、独立查询读取会话、按面板加载工具明细及隐私刷新票据修复。[分层验证](docs/acceptance/panel-read-query-tools-scoped-2026-10-19.json)中的本轮126项Core、113项Desktop与61项前端测试分别通过，不代表Native或性能达标。[原型记录](docs/acceptance/custom-installer-2026-10-19.json)和[正式入口R3复用封装](docs/acceptance/windows-installer-entry-2026-10-19.json)及其[release](output/build-candidates/windows-installer-orchestration-20261019-r3/release/)保留为历史，旧失败与预览资格不改绑到C40。

猫娘保留猫耳和猫尾，使用正常的人类手脚。2026-10-19主源码默认使用[v12新素材包](assets/pets/nekomi/manifest-animation-v12.json)：63张1024×1536透明PNG、16个动作片段，增加向左侧走与左右掉头。原图、提示词与来源保存在 `assets/pets/nekomi/walk-v10-complete/` 和 `animation-v6/`；仅打包入选整角色PNG，固定画布和共享锚点。长片段按最多12张图的窗口串行加载，加载期间暂停播放时钟；持续工作/提醒只循环主体，空闲动作和提示按完整时长播放。减少动画显示静止待机，免打扰显示静止坐睡，隐藏与来源切换取消旧资源。分层素材保留作后续制作，不进入运行包。

v12源码接入已通过前端239项测试、类型检查、前端构建及分发来源生成；16动作的真实浏览器播放与减少动画、动作切换、收起恢复、PNG请求失败停止及显式重播检查通过，详见[接入记录](docs/acceptance/pet-animation-v12-integration-2026-10-19.json)。偏好设置提供独立动作预览，收起后释放资源；两段掉头各保留32步、23张原图与6560毫秒呈现时长。素材仍为有容差的离散关键帧基线，不具备严格无滑步资格。侧走和掉头均原地展示，Native游走权限保持关闭；v12已进入当前C41安装器候选，完整原生、资源和发布验收仍未完成，C33等封存候选保持历史归属。

历史完整三产物[C39事件顺序修复候选](docs/acceptance/c39-current-build-2026-10-19.json)：本轮完整Windows App／NSIS／headless CLI与独立静态后核通过，456项源码、2705项只读审核、719条校验闭合。23项纯测试精确绑定四份冻结源，Windows分支另由完整App编译；App／CLI非自然自有编译器后代清理、其余四阶段自然结束。此静态回执只记构建资格，本轮有限Native结果见下文，完整拖动和M4另验。

[C39有限原生收尾](docs/acceptance/c39-ui-native-scoped-2026-10-19.json)：基础短拖因入口已松键安全拒绝，不算位移通过；展开卡片后一次短拖实际移动-80／-80，最终1544／596／petScale1保存通过；正常菜单退出root0／Job0、15／15同句柄退出，独立观察15项与闭库7项／51守卫通过，未保存临时弹层几何。C37／C38失败保留，完整手动拖动、Native、M4及全V1仍未通过。

历史[C38静态候选](docs/acceptance/c38-current-build-2026-10-19.json)与下方有限原生回执保持原范围；展开拖动失败未抹去。

[C38有限原生收尾](docs/acceptance/c38-ui-native-scoped-2026-10-19.json)：基础短拖实际移动-80／-80；展开卡片后的04短拖与06长拖都只收起弹层、未进入新的原生拖动，失败保留。本次正常菜单退出root0／Job0、18／18同句柄退出，闭库7项／54守卫通过，最终位置1544／596／petScale1且未保存临时弹层几何。完整手动拖动、Native和M4未通过，C39修复现已进入上方静态候选，原生复测另记；C38不是最终交付。

历史三产物[C37当前UI与v12候选](docs/acceptance/c37-current-build-2026-10-19.json)：456项冻结源码包含小面板临时加高、手动拖动与最新滚动条，63张原PNG／16动作／33份生成来源，67项dist、644项法律资源，14项静态检查与719条分发校验一致；独立后核2709项只读输入全部闭合。本轮六阶段均root0／Job0，App和headless CLI按预声明清理自有编译器后代、非自然整Job结束，其余四阶段自然结束。实际安装器payload与预编译EXE只差默认三字节标记，编译根EXE已恢复；完整Native UI、生命周期、性能、安装和完整V1仍未验收。

[C37有限原生UI与闭库核对](docs/acceptance/c37-ui-native-scoped-2026-10-19.json)已完成：50%／100%／200%卡片及100%菜单实际打开与关闭通过；本次真实菜单退出为同句柄root0／Job0、18／18身份退出、receiver冻结并join、owner句柄关闭。闭库独立7项核对、76只读守卫通过：9份合成来源3050 token／8响应，最终位置1360／352、petScale1，未保存520高临时弹层几何。04快速拖动未移动窗口，失败保留；手动拖动、完整Native、性能、安装和M4仍未通过，后续C38修复待原生复测。

历史三产物[C36R1 v12组合候选](docs/acceptance/c36r1-v12-composite-build-2026-10-19.json)：454项冻结源码、63张原PNG／16动作、67项dist和644项法律资源，14项静态检查与718条分发校验一致，独立后核通过。它复用原C36成功的Rust编译和NSIS生成，提取实际安装器payload并验证仅有正常的三字节标记差异，再构建headless CLI并封存；原App外层root1的错误后核保留，CLI仍为root0后按预声明清理自有编译器残留、非自然整Job结束。这不是一次全新六阶段构建，也不代表安装或发布合格。

C36R1 Default已实际运行9文件合成来源；[关闭后数据库与固定日期CSV独立审计](output/implementation/c36-default-closed-audit-20261019-r3/audit.json)11组通过。固定上海日2026-10-18为2950 token／7响应，2026-10-17为100／1，样本全期3050／8；这不是用户真实Codex总用量，跨日后不能将固定日期称为“今日／昨日”。该次同句柄root0／Job0、18／18身份退出且最终receiver已停止；只证明该次有范围的运行与退出事实，不代表完整生命周期、动态动画、性能或M4通过。

C36R1 Development曾观察到本地皮肤导入／选择／回内置及临时交互等步骤，但原guardian因退出证明到期保持unknown，最后仍有8个活动成员；之后仅证实精确根进程非零退出。后续[只读取证](output/implementation/c36-development-unexpected-exit-forensic-20261019-r2/evidence-summary.json)保留DB／WAL／SHM、9文件事实、52张导入PNG及最终有效设置，不能回填正常退出、Job0、温启动或P1原生验收资格。

C37阶段已纳入[大面板滚动条、小面板临时加高与手动拖动改动](docs/acceptance/pet-panel-drag-ui-2026-10-19.json)：前端构建和浏览器布局检查通过，纯几何8项与手动拖动12项测试通过；浏览器窗口大小是显式模拟，不能证明Windows HWND扩窗。含这些改动的App已在C37构建封存，完整Native UI、手动拖动和发布资格仍未通过；本次卡片／菜单和正常退出的有限通过见上文新回执，C36R1证据不自动改绑当前UI。

当前开发基线是[开发方案 v0.3](开发方案.md)。本目录仍在开发；是否可发布以[验收追踪](docs/acceptance/v0.3-验收追踪.md)和实际证据为准。

源码仓库包含代码、角色素材、许可补充文件和构建必需的11份素材来源记录；`output/`中的其余构建包、数据库、运行日志及本机验收原始证据保留在开发机，不随Git提交。验收摘要中的本机证据路径用于追溯，并非仓库内可下载文件。`.gitattributes`保留文件原始字节，避免换行转换破坏素材来源摘要。

2026-10-18 追加的本地扩展开发包含 PNG 目录包导入/预览/显式选择与回退、保守透明区域命中、15 秒临时交互。[开发记录](docs/acceptance/p1-local-extensions-development-2026-10-18.json)中的该轮前端225项、真实模块测试18项、前端构建和桌面源码类型检查通过；接口和开发开关见[本地 PNG 皮肤目录包 v1](docs/contracts/local-png-skin-v1.md)。这三项普通启动均未开放；C36R1已包含开发入口，但Development有限观察及取证不表示P1原生、资源或发布合格。

后续已补强[原生暂停边界](docs/acceptance/p1-native-runtime-boundary-2026-10-18.json)：锁屏、睡眠等暂停以及停止/退出会撤销旧掩码和临时交互，暂停期间拒绝发布、续租和临时交互，恢复后需重新握手。纯策略9项测试通过；该轮Desktop源码的release类型检查已通过（root0、自然Job0、165项输入核对关闭），暂停准入刷新接线已纳入冻结C36。HRGN实际恢复仍沿既有UI tick，原生、多屏/DPI、真实睡眠和发布验收继续待完成；当前后续UI改动不沿用该类型检查结论。

2026-10-18 V1.0续验收：前端全量220项通过；[C34 App构建](docs/acceptance/c34-v11-v1-build-failure-2026-10-18.json)因全机提交内存耗尽失败，原记录保留。另行构建的[当前headless CLI与受限Core核对](docs/acceptance/c34-headless-core-scoped-2026-10-18.json)通过：实际首次导入和重放均自然exit0/Job0，手工样本全期3050 token／8响应，独立SQLite分期为今日2950／7、昨日100／1；计量、工具、历史收件箱和隐私标记检查通过，重放事实保持稳定。独立复核已完成；该结果不代替日期查询界面、新App／安装器、原生交互、性能和M4验收。

后续以C33旧包补做了[不变数据组件的原生界面检查](docs/acceptance/c33-core-ui-native-scoped-2026-10-18.json)：今日2950／7、近7天与30天3050／8、自定义昨日100／1，以及细分、会话、工具、收件箱、健康与昨日CSV；关闭后的数据库与CSV只读复核11组通过。旧包仅用于该组件范围，不能代替v11动画、当前性能或安装验收。两次启动失败均保留；隔离用户目录补齐后才成功运行。实际菜单退出为root0／Job0，但子进程身份12／18存在缺口，完整生命周期仍未合格。另修复了[显式数据目录仍提前读取默认目录的问题](docs/acceptance/data-dir-lazy-fallback-2026-10-18.json)，实际原表达式4通过／2失败、修复表达式6通过。该惰性选择修复后来已纳入冻结C36R1；原6例源级证明保持原范围，不据一次启动扩大为完整Native修复验收。

历史完整三产物[C33候选](docs/acceptance/c33-v6-panels-headless-build-2026-10-17.json)，[独立后核R1](docs/acceptance/c33-v6-panels-headless-post-audit-2026-10-17.json)通过。新冻结源码纳入Panels两项修复及独立headless CLI入口，保留v6的38帧/12动作；809项源码含本轮生成的526项法律资源，42项dist前后稳定，14项静态检查与588项分发校验通过。产物路径、大小与SHA见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。

该C33轮六阶段最终均root0/Job0且守卫关闭；App和CLI按预声明清理自有编译器残留，非自然整Job结束，其余阶段自然完成。封存当时App、安装器及生产CLI均未运行或安装；本轮后续C33原生局部检查见上文，安装器仍未执行。真实账户/桥接和模型未调用。构建不扩大旧37/40/R3、Panels9组/33观测或headless分次8语义组/21Core的原范围；完整Native、安装、性能、合格步态与M4仍未通过，不据此证明内存改善。C32的CLI失败和C31R1历史三产物继续保留。

主源码已完成[离线CLI特性接入与受限验证](docs/acceptance/headless-cli-feature-integration-2026-10-17.json)：正常App默认使用desktop，custom-protocol仍包含desktop；离线ledger_audit使用独立headless-audit并关闭默认特性。六项GUI依赖改为可选，Core、账户桥接、通知门、时钟、完整例程及迁移的39项原输入保持原字节，Cargo.lock未改。八组语义检查分次完成，另有21个临时纯Core用例通过；原37项中的16个desktop用例未在本轮运行，不计入通过，旧owner超时、诊断解析失败及跨通道反例均保留。

新的headless CLI已编译归档，采用实际CGU16/jobs1、O3/thin和strip=debuginfo；这不证明内存改善，也没有运行生产CLI、App、安装器或真实账户/模型。该接入回执记录当时的独立CLI验证；三文件与下述Panels修复随后已纳入C33。C32仍是历史部分构建，C31R1三产物保持原样。

历史[C32小卡候选部分构建](docs/acceptance/c32-v6-pet-card-shortcuts-partial-build-2026-10-17.json)已归档App与安装包，纳入v6及小卡快捷入口；配套CLI因LLVM内存不足失败，未执行最终封存、14项静态检查或完整分发校验，因此不是完整三产物候选。App阶段自然退出0、Job归零，809项源码、34项控制与42项dist守卫已关闭。产品路径和校验值见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。C32冻结产品仍保留首次单边设置定时免打扰与会话明细旧统计两项缺陷；主源码的后续修复及受限检查见下文，未回写本包。

主源码已应用[定时免打扰与会话明细修复](docs/acceptance/panel-dnd-session-refresh-2026-10-17.json)：时间两端成对保存、保存中均不可编辑；会话明细按来源＋会话身份读取最新快照，记录移除后关闭且不自动重开，并保留项目映射草稿。旧行为已实际复现，修复后9组真实组件配合有限合成host的检查、33次断言观测及受保护输入的类型检查通过。33次观测包含诊断，不是33个场景；这不证明App.save、Core持久化/调度或Native端到端。修复已纳入C33，C32与C31R1均不含该修复；该组件回执、既有37/40代码及R3小卡证据仍保留原输入范围。

历史[C31R1组合候选](docs/acceptance/c31r1-cli-recovery-composite-build-2026-10-17.json)：复用C31已归档App和NSIS，新构建配套CLI，独立后核见[审查记录](docs/acceptance/c31r1-cli-recovery-post-audit-2026-10-17.json)。恢复副本的804项源码、42项dist与原C31逐字节等价，48项App归档一致，14项静态检查和575项分发校验通过；这不是六阶段全新重跑。三产物路径、大小与完整SHA见[候选发布说明](docs/release/0.1.0/候选发布说明与已知限制.md)。

新CLI采用根包CGU16、jobs1，保留O3/thin LTO；CLI与seal分别用时231.1566266秒、5.9023174秒，均自然结束、root0/Job0，守卫全部关闭。沿用App仍保留其原root0后清理自有残留、非自然整Job结束的事实；[原C31 CLI内存分配失败](docs/acceptance/c31-v6-functional-reliability-partial-build-2026-10-17.json)没有被改写。实际根包编译参数另观察到`strip=symbols → debuginfo`，不能称仅CGU改变，也不能因本次成功证明单参数降低内存。所有C31R1产品均未执行或安装，Native、性能与完整M4继续后置。

C31R1包含C31冻结时的[工具/会话/查询修复](docs/acceptance/v1-functional-reliability-2026-10-17.json)与[预算实时许可修复](docs/acceptance/f08-budget-live-permits-2026-10-17.json)；预算证据仍为保留68项通过并补4项的组合，不能称一次全69项重跑。后续统计小卡的“查看今日未读”和“返回最近聊天”已完成[代码与受限浏览器检查](docs/acceptance/pet-card-shortcuts-2026-10-17.json)：后端37项、前端40项及12组真实React界面配合合成IPC的交互分别通过，R3夹具另经双页40次并发写入和负例检查；R1/R2失败与旧代码回执原样保留。新小卡已进入C32部分构建的冻结源码，未纳入C31R1；C32的App与安装包已归档，CLI失败且未封存，该时点完整三产物仍为C31R1；当前C41重封安装器候选见上文，C33保留历史归属。这些检查不代表Native、系统剪贴板、真实聊天定位、安装、性能或完整M4通过；真实走动仍缺合格步态。

v6源码接入的51项定向测试、类型检查和前端构建通过，12动作的浏览器展示、透明深浅底、暂停恢复及来源切换取消已有受限检查。[接入记录](docs/acceptance/pet-animation-v6-integration-2026-10-17.json)说明各次验证范围；[前端目录](output/build-candidates/animation-v6-web-20261017/dist/)仅为该阶段网页构建，Windows历史C33候选见上文，当前安装器候选为C41，原生交互、安装和性能验收继续后置。

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

干净克隆在构建 Windows App 或安装包前还需生成分发许可目录。先下载锁定的 Cargo 依赖，并备齐 `%LOCALAPPDATA%\tauri\NSIS` 下的 NSIS 3.13（含 `COPYING`）与 `nsis_tauri_utils` 0.5.3 插件；`scripts/distribution-manifest.mjs`会核对这些本地工具的版本和摘要。生成的 `output/distribution-licenses/` 是 Tauri 资源输入，不纳入 Git；11份已跟踪的素材来源记录必须保持原始字节。

```powershell
pnpm build
cargo fetch --manifest-path src-tauri/Cargo.toml --locked
```

`pnpm tauri build`现在只生成内部静默引擎，位于实际Cargo target的`release/bundle/nsis/`，指定target时还包含目标架构子目录；它不能作为用户安装入口直接交付。正式入口为`pnpm run build:windows`，使用PowerShell 7和本机已准备的7z工具：

```powershell
$setup7z = 'C:\Program Files\7-Zip\7z.exe'
$setup7zDll = 'C:\Program Files\7-Zip\7z.dll'
pnpm run build:windows -SevenZipPath $setup7z -SevenZipExpectedSha256 (Get-FileHash -LiteralPath $setup7z -Algorithm SHA256).Hash -SevenZipDllPath $setup7zDll -SevenZipDllExpectedSha256 (Get-FileHash -LiteralPath $setup7zDll -Algorithm SHA256).Hash
```

该入口默认生成许可资源，在本项目`output/`内新建独立Cargo target构建Windows x64引擎，再从实际引擎提取应用和许可来源、核对产品版本与三份安装器源码，最后封装自绘界面。只交付输出中的`release/`：自绘安装程序、`SHA256SUMS.txt`和回执。终端会输出实际路径；`work/`与原始NSIS引擎保留作构建证据。7z、PowerShell和开发工具不进入用户安装包。复用已核验引擎及仅检查工具路径的方式见[安装器构建说明](installer/README.md)。未签名包如实标记分发限制；生成安装包不代表安装或发布门槛通过。

开发与验收可以用绝对路径环境变量 `TOKEN_MEOW_DATA_DIR` 指定独立的宠物数据目录；它不改变 Codex 源目录。请勿把宠物数据目录设置到 Codex 源目录中。

离线账本回放可使用 `ledger_audit` example，只读账户与分页目录可使用 `account_bridge_audit` example。后者只保留脱敏统计，不输出邮件、会话标题或路径，不发起模型推理、登录或任务控制。示例的证据文件应写入本项目 `output/`；原生窗口与发布验收另行执行。

从当前源码构建离线账本工具使用以下命令；离线环境可另加`--offline`。构建不等于执行回放或完成发布验收。

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --example ledger_audit --release --no-default-features --features headless-audit --jobs 1 --config profile.release.package.token-meow.codegen-units=16 --locked
```

## 边界与后续

首版不控制 Codex 执行，不审批、不切模型、不压缩会话、不修改登录或源配置，不安装 Hooks/遥测桥接，不收集全局输入内容。Claude Code/Gemini 仅为后续适配器；语音视觉聊天、养成、任意代码 MOD、用户皮肤导入、Live2D/VRM 均有独立开放条件。

本项目没有打包调研项目的角色或完整运行时。原创角色与代码依赖分别记录来源、许可和摘要；参考资料见[开源桌宠调研](docs/research/开源桌宠调研-2026-10-15.md)。
