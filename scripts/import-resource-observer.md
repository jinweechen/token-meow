# 完整首导入观察协议

`start-import-resource-job.ps1` 是独立的 `full-first-import-v1` 协议。它不改动原来的固定 600 秒观察脚本，也不会产生 idle 资格。执行不带 `-SelfTest` 的命令会启动选定应用；当前开发阶段只执行了无应用启动的自检。

先由已经核实的 production Core prepare-only 路径，在全新的数据目录中准备 schema 和静态偏好，并保存 `scanCalls=0`、`snapshotCalls=0` 的 prepare receipt。输入必须是私有冻结清单中的完整 JSONL 目录，不能把它或任何原始日志放入 delivery。

```powershell
.\scripts\start-import-resource-job.ps1 `
  -Executable '<本轮候选 EXE 绝对路径>' `
  -ExpectedSha256 '<本轮候选 EXE SHA256>' `
  -SourceRoot '<私有冻结目录\source>' `
  -SourceManifestPath '<私有冻结目录\manifest.json>' `
  -ExpectedSourceManifestSha256 '<冻结清单 SHA256>' `
  -DataDirectory '<本轮全新 prepare-only 数据目录>' `
  -PrepareReceiptPath '<本轮 prepare receipt.json>' `
  -OutputDirectory '<output\performance 下尚不存在的本轮观察子目录>'
```

协议固定最多 3600 秒、每 1 秒采样、最大合规采样间隔 2.5 秒、采样私有内存峰值 600 MiB。Job 在 root 进程恢复执行之前完成分配，不允许 breakaway，采样器及 Python sidecar 在应用 Job 之外；复用的 `resource-job.cs` 与原协议相同。首个样本在恢复执行之前，全部子进程都计入 Job。CPU 和平均内存仅描述本轮导入，不套用 idle 的 1% / 350 MiB 门槛。

启动前核实 EXE、清单、所有 frozen input 文件哈希；prepare receipt 的源和数据目录按实际目录身份比较，SQLite 中必须恰好一个启用的 Codex 来源、显式 primarySourceId 一致，所有采集事实表仍为空。启动后 sidecar 绑定 PID、creation FILETIME、运行 image 与 Job，拒绝旧进程、旧指标、未来时间、缺失主来源或换源。

完成条件是：清单中每个文件都已登记，historical offset 等于冻结完整字节数，generation 未变化，历史与 live 的 discard/bootstrap 已结束，`historyPending=0`、`pendingFiles=0`，且未暂停或取消。首次满足是基线，之后还须三次不同且推进的 fresh collector 成功迭代。为避免猜测未落盘的迭代，实际确认可能受空闲 metrics 最多 30 秒落盘节奏影响。之后在父 Job 继续采样期间再次以 2 MiB 块哈希全部输入，再确认真实进程与游标状态；只看相同 length 不算输入未变。

任何已知跳过、解析问题或 unsupported 来源都明确使 coverage 不完整。达到 EOF 的扫描过程可以结束，但不能追认为完整事实覆盖。driver 只有在 completion receipt 的协议、Job、PID、creation 全部匹配，且 sidecar 正常以退出码 0 结束时才接受完成资格。

任何合规样本已超过 600 MiB 时，会立即以 `failed-observed-import-memory-threshold` 停止观察并保留采样反例，未完成的导入仍为 `completionQualified=false`。超时、观察错误、进程早退或超过 2.5 秒的 gap 都不能通过。发生 gap 后不删除样本，也不重新挑选一个看起来更好的区间。

`configuredStatic`、`ordinaryRuntime`、`mainHidden`、`petVisible` 来自本轮实际偏好和原生 metrics，仅作为条件记录；偏好值不能替代实际 renderer 的原生观察。停止观察会留下已恢复执行的应用，供操作者正常退出；只会清理自己的 Python helper。启动恢复之前失败时，沿用既有 Job 类仅回收新建的 suspended root。

输出 `plan.json`、`preflight.json`、`launch-binding.json`、`import-job.jsonl`、`import-state.jsonl`、`completion.json` 和 `summary.json` 均在新的观察目录中。错误/中断没有完整 end receipt 时仍属不合格，不能根据中途某一条看似合格的数据判通过。

```powershell
# 仅新建私有 SQLite/文件协议 fixture，不启动任何应用：
.\scripts\start-import-resource-job.ps1 -SelfTest
```
