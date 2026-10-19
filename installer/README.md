# Token 喵自绘安装器

安装器使用独立的 WPF 窗口，延续应用的奶油白、鼠尾草绿和小柚角色。窗口负责展示与收集选项；安装服务负责校验、运行内嵌 NSIS 引擎和核验安装结果。内部引擎以静默方式工作，不再弹出原生 NSIS 安装向导。

目标环境为 **Windows 10／11 x64、.NET Framework 4.8 或更高版本**；本次开发机器为 4.8.1。安装器界面自身不使用 WebView2。Token 喵应用仍需要 WebView2；设备缺少该运行时的情况下，安装微软组件的步骤需要联网。

当前推荐下载[C41自绘安装包](../output/build-candidates/c41-current-v1-20261019/release/TokenMeow_0.1.0_x64-custom-setup.exe)，128,791,040字节，SHA-256 `7CB3F71A16D37E756026D4B9132485B8BA5F33C9CDA9D3E0A1F5FF079E035190`。同目录仅另附校验和与回执。[C41重封记录](../docs/acceptance/c41-webview-execwait-repackage-2026-10-19.json)记录两处WebView2可执行路径引用及启动错误门修复，8个隔离分支实际用例、20项静态检查通过；引擎重封与WPF封装两Job均自然root0／Job0、0终止。App沿用C40实际NSIS payload原字节，没有Rust／App或headless CLI重编。安装包未签名，界面视觉、真实安装、WebView2下载、升级、卸载、Native及性能尚未验收，整个V1仍未完成。

## 界面与实际行为

| 界面 | 实际行为 |
| --- | --- |
| 欢迎页 | 展示本机 Codex 只读用量账本、会话／工具与收件箱提醒、安静桌宠陪伴；不宣传尚未开放的 P1 功能。 |
| 安装位置 | 默认使用当前用户应用目录，可在折叠区域直接输入完整路径；已有受支持的安装只允许在原目录升级，本轮不迁移目录。 |
| 快捷方式 | 一个总开关控制桌面和开始菜单快捷方式，没有两个独立开关。 |
| 安装中 | 显示宿主报告的三个阶段和不定进度条，不推算或伪造百分比。真实安装期间禁止关闭窗口，也不会强制终止引擎或正在运行的应用。 |
| 失败页 | 在窗口内显示错误，提供重试及返回修改路径；不使用原生 MessageBox。 |
| 完成页 | 引擎返回成功且应用及安装记录核验通过后展示；勾选“完成后打开”仅改变主按钮行为，必须点击“完成并打开”才请求启动。也可以仅完成并关闭。 |
| 启动失败 | 保留应用已安装的事实，在完成页显示启动错误；不会把启动失败改写成安装失败。 |

真实阶段按顺序为：

1. **校验安装环境**：检查目录、应用运行状态和内嵌引擎。
2. **正在安装 Token 喵**：由 NSIS 引擎处理应用及所需运行时。
3. **验证安装结果**：核对已安装应用 SHA-256 和当前用户安装记录。

关闭、最小化、Tab 焦点及标题区拖动由 WPF 窗口处理。预览模式允许随时关闭，包括固定的安装中预览；这不放宽真实安装时的关闭限制。

## 源码分工

| 文件 | 职责 |
| --- | --- |
| [InstallerWindow.cs](TokenMeow.Setup/InstallerWindow.cs) | 无边框自绘窗口、欢迎／进度／错误／完成状态、路径和选项、安装中关闭限制。 |
| [SetupContracts.cs](TokenMeow.Setup/SetupContracts.cs) | 环境、选项、进度和结果四个 DTO。 |
| [Program.cs](TokenMeow.Setup/Program.cs) | WPF 入口、预览参数解析、UI 与服务事件连接。 |
| [InstallerService.cs](TokenMeow.Setup/InstallerService.cs) | 预览分支、真实安装编排、引擎和应用 SHA 校验、安装记录核验、显式启动。 |
| [InstallPolicy.cs](TokenMeow.Setup/InstallPolicy.cs) | 路径与原目录升级策略、内部引擎参数和退出码说明。 |
| [InstallerNative.cs](TokenMeow.Setup/InstallerNative.cs) | Windows 原生文件／目录防护及进程运行状态查询。 |
| [app.manifest](TokenMeow.Setup/app.manifest) | 当前用户权限、Windows 兼容性和 DPI 声明。 |
| [build-custom-installer.ps1](../scripts/build-custom-installer.ps1) | 校验并保持构建输入、生成 BuildInfo 与程序集信息、调用系统 C# 编译器、生成构建回执。 |
| [installer.nsi](../src-tauri/windows/installer.nsi)、[installer-hooks.nsh](../src-tauri/windows/installer-hooks.nsh)、[installer-engine.nsh](../src-tauri/windows/installer-engine.nsh) | 当前用户 x64 隐藏安装引擎、安装准入检查及原有普通卸载入口。 |

构建不依赖 NuGet 或 XAML 编译，使用系统 .NET Framework C# 编译器和 WPF 程序集。`BuildInfo.cs` 在输出目录生成，绑定产品版本、引擎资源名及 SHA、应用预期 SHA 和角色资源名。角色原图来自已入选的 [master-front-v2.png](../assets/pets/nekomi/walk-v10-complete/model-reference/master-front-v2.png)，原字节嵌入，不生成新图。

## 构建

正式构建入口为`pnpm run build:windows`，等价于PowerShell 7执行[scripts/build-windows-installer.ps1](../scripts/build-windows-installer.ps1)。默认`Build`模式先生成许可目录，再以新建的本项目独立Cargo target构建Windows x64 NSIS引擎（jobs1、根包CGU16），最后从实际引擎提取payload并封装WPF；不会执行安装器或应用。历史C40已完成此默认源码完整构建；C41仅重封修复后的NSIS引擎并完成WPF封装，不能记为又一次App源码构建。工具预检与历史复用封装仍按各轮原范围记录。

开发机须提供7z可执行文件及其SHA；若有相邻`7z.dll`，也必须提供DLL路径及SHA。7z用于流式提取实际`token-meow.exe`和`licenses/manifest.json`，不修改共享Cargo输出，也不假定Tauri的三字节标记。脚本核对唯一NSIS档案身份、实际应用x64与版本、许可清单中模板及两份supporting source的当前SHA，才调用下述外层编译器。pnpm的工具路径先解析到本项目内的实际普通文件并核对版本，不放宽重解析路径检查。

完整参数示例见[项目README](../README.md)。可增加`-OutputDirectory`指定尚不存在的本项目`output/`子目录。`-PreflightOnly`只检查本机Node／Tauri实际路径、版本及输入守卫，不运行Cargo、许可生成或安装；`-Mode ReuseEngine -EnginePath <实际新引擎路径> -EngineExpectedSha256 <实际64位SHA>`跳过Rust构建，仅封装已核验的新引擎。两种方式仍需传入7z参数。

只分发新输出的`release/`目录，其中恰好包含自绘setup、SHA256SUMS和回执；`work/`、Cargo target和`bundle/nsis/`都是内部证据。新隐藏引擎直接双击会退出64，不能当作最终安装包。该开发流程不会给用户额外增加7z、PowerShell、Node或Rust依赖。

先准备经过审核的本轮隐藏 NSIS 引擎和与其匹配的应用 EXE，再运行仓库根目录的构建脚本。以下是**概念路径与占位 SHA**，需要替换后才能执行：

```powershell
pwsh -File .\scripts\build-custom-installer.ps1 `
  -EnginePath 'D:\approved\current-engine\TokenMeow_0.1.0_x64-engine.exe' `
  -EngineExpectedSha256 '<已审核引擎的64位十六进制SHA256>' `
  -ApplicationPath 'D:\approved\current-app\token-meow.exe' `
  -ApplicationExpectedSha256 '<已审核应用的64位十六进制SHA256>' `
  -OutputDirectory 'D:\build-output\token-meow-custom-setup-new'
```

五个参数均为必填：

| 参数 | 输入要求 |
| --- | --- |
| `EnginePath` | 使用本轮安装脚本构建的隐藏 NSIS 引擎文件。不能用旧的普通安装包替代。 |
| `EngineExpectedSha256` | 审核确定的引擎 SHA-256，64 位十六进制；脚本将与实际文件比较。 |
| `ApplicationPath` | 引擎所安装、后续需要核验的匹配应用 EXE。此文件提供应用哈希绑定，不作为第二份应用资源另行嵌入。 |
| `ApplicationExpectedSha256` | 审核确定的应用 SHA-256，64 位十六进制。 |
| `OutputDirectory` | 全新的输出目录，其父目录必须存在。脚本拒绝覆盖旧输出，应保留失败记录并另选目录。 |

脚本只构建，不执行安装器、引擎、应用或卸载器。输出包含 `TokenMeow_<版本>_x64-custom-setup.exe`、生成源码、编译参数／日志、`source-inputs.json` 和 `build-receipt.json`。回执记录构建输入与输出 SHA、编译结果及输入句柄关闭情况；脚本本身不替代引擎 payload 与所选应用一致性的独立审核。

当前App沿用[C40完整源码构建](../docs/acceptance/c40-full-source-custom-installer-2026-10-19.json)的实际NSIS payload：137,782,272字节，SHA-256 `6EBAE13471CD2A74F79AAD7F91DA13C693B2868D1F62A8858E45881C1D43C15C`。它与当时编译目录中的base EXE仅有唯一27字节标记末三字节的正常`UNK`→`NSS`差异，其余字节完全相同，见[实际变体绑定](../output/implementation/c40-full-source-installer-20261019-r2/variant-binding.json)。C40旧引擎不含本次WebView2修复，其18项静态检查与非自然自有后代清理收尾保留历史范围。后续针对该App的Native验收仍应绑定实际payload；watcher窄配置读取新源码尚未进入此App，不能声称已有性能收益。覆盖提示、查询读取会话、按需工具明细及隐私票据修复已包含；[R5分层证据](../docs/acceptance/panel-read-query-tools-scoped-2026-10-19.json)中的126项新Core、113项Desktop与61项前端测试不等于安装或性能验收。

历史原型[TokenMeow_0.1.0_x64-custom-setup.exe](../output/build-candidates/custom-installer-shell-20261019-r1/TokenMeow_0.1.0_x64-custom-setup.exe)为128,812,544字节，SHA-256 `9E12941DD45CF5CBD9FFE226F0AE968F8464CA4EFE23CBAF5F9E6BE5E89A6AE2`。外层资源核对确认实际嵌入新引擎及原角色图，新引擎645个payload文件核对通过，该轮应用复用C39实际NSS原字节，并非旧C39安装包。18项后端测试通过；该轮预览被Windows控制工具以应用未授权拒绝，屏幕布局待人工确认。具体资格与保留失败见[原型回执](../docs/acceptance/custom-installer-2026-10-19.json)，不代表实际安装通过。

历史[R3复用封装候选](../output/build-candidates/windows-installer-orchestration-20261019-r3/release/)的setup为128,812,544字节、SHA-256 `52233A1EDADDC04ED64A69D8471A64994471F1AC429C43379A55CFFEB088A399`，同目录附校验文件及回执。[正式入口验证](../docs/acceptance/windows-installer-entry-2026-10-19.json)记录真实pnpm工具预检、8项有限负例与管道检查、实际复用封装以及独立资源核验；该轮没有运行默认完整Rust构建或实际安装。原型、R3及旧C39的静态或有限Native证据均保留原范围，不能改绑为当前C41的实机界面或安装验收。

## 本地预览

对构建出的自绘安装器显式传入预览参数：

```powershell
& 'D:\build-output\token-meow-custom-setup-new\TokenMeow_0.1.0_x64-custom-setup.exe' --preview
& 'D:\build-output\token-meow-custom-setup-new\TokenMeow_0.1.0_x64-custom-setup.exe' --preview=progress
& 'D:\build-output\token-meow-custom-setup-new\TokenMeow_0.1.0_x64-custom-setup.exe' --preview=success
& 'D:\build-output\token-meow-custom-setup-new\TokenMeow_0.1.0_x64-custom-setup.exe' --preview=error
```

`--preview` 打开欢迎页，按钮可演示预览流程；另外三个参数直接停留在相应固定状态，便于检查布局、文本和关闭行为。预览有明显标识，不释放或执行安装引擎、不安装应用、不写 HKCU、不创建快捷方式、不启动 Token 喵，也不进行 WebView2 下载。预览完成页只表示界面流程展示完成。省略预览参数会进入真实安装模式，点击安装后可产生实际安装变更。

## 内部引擎结果

宿主使用 `/S` 调用当前用户 x64 引擎；关闭快捷方式总开关时附加 `/NS`，安装目录通过末尾的 `/D=` 传入。这是宿主内部协议，用户应运行外层自绘安装器。

| 退出码 | 含义与处理 |
| --- | --- |
| `0` | 引擎报告完成；宿主仍需核验应用哈希和安装记录，不能仅凭此码展示安装成功。 |
| `1` | 安装中止，未通过外层完成核验。 |
| `64` | 内部引擎未以 `/S` 启动，直接退出，不弹出原生安装向导。 |
| `65` | 目标应用仍在运行；请通过应用正常退出后重试，安装器不会强制关闭它。 |
| `66` | 无法确认相关进程状态，按 unknown 停止，不能当作应用已退出。 |
| `67` | 不支持的安装身份、目录或类型，包括不兼容的 identity／location、机器范围安装或 WiX 安装；保留原安装并检查。 |
| `68` | 已安装版本更新，拒绝降级。 |
| `69` | WebView2 获取或安装失败；检查网络及运行时安装条件后重试。 |

其它非零码同样作为未完成结果在自绘窗口内显示。引擎不会通过旧卸载器链或强制关闭进程来绕过运行状态检查。

## 卸载与验收边界

普通卸载仍使用现有 Windows 已安装应用入口及原 NSIS 卸载界面，保留原有数据选择。当前没有自绘维护器，也没有将整个卸载流程改为自绘。

源码完成、编译成功和预览检查分别证明相应范围；它们不等于真实安装、原目录升级、快捷方式、缺失 WebView2 的联网安装、启动或卸载均已通过。上述操作需要对本轮实际产物分别留存验证记录。旧 C39 App 的有限验收可以按原范围保留，新引擎和外层安装器仍须独立验证；不能据此宣告完整 Native、安装发布或整个 V1 完成。
