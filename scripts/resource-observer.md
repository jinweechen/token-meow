# Windows resource acceptance observer

The application and Core are unchanged by these scripts. The observer is an independent process outside a new Windows Job. It creates only the explicitly named target, suspended, assigns it to the Job, and resumes it. Breakaway is disabled; nested application jobs remain contained. Closing the observer leaves a running application alone. A launch failure before Resume terminates only its newly created suspended target.

The owner of native acceptance runs the following commands **after closing its prior application instance**. Do not run a second application instance against the same database. Use an absolute executable path, an expected build SHA-256, and a new evidence filename.

```powershell
$petExecutable='D:\absolute\path\token-meow.exe'
$petExpected=(Get-FileHash -LiteralPath $petExecutable -Algorithm SHA256).Hash
./scripts/start-resource-job.ps1 -Executable $petExecutable -ExpectedSha256 $petExpected `
  -WarmupSeconds 60 -DurationSeconds 600 -SampleMilliseconds 1000 `
  -OutputPath ./output/performance/idle-job-unique.jsonl
./scripts/measure-resources.ps1 -JobMetricsPath ./output/performance/idle-job-unique.jsonl `
  -DurationSeconds 600 -Scenario idle
```

Choose and record the warmup window before the run. The native acceptance owner separately verifies that the import has ended, details are closed, and the configured pet is static or uses low frequency animation. A long warmup alone does not prove those conditions. An interrupted/error JSONL cannot qualify. No import, lifecycle, short fixture, or legacy PID observation substitutes for the ten-minute idle gate.

`totalUser100ns + totalKernel100ns` is cumulative Job CPU, including children that exited between samples. The measured CPU interval is the difference between baseline and endpoint, divided by measured monotonic seconds and all 12 frozen logical processors. Warmup CPU is subtracted with the baseline. The full observed span must reach the requested 600 seconds; 598 seconds fails qualification.

Current private bytes, working set and handles are summed from live Job members. Each member is opened by handle and rechecked against the actual Job before reading, so PID reuse cannot include an unrelated process. An inaccessible live member makes coverage incomplete and prevents qualification. Private memory uses a time weighted trapezoidal sample mean and records the sampled maximum. Samples over 2.5 seconds apart invalidate the one-second sampling run. Job lifetime peak commit is separately labelled and never substituted for current private memory. `terminatedProcesses` is Windows limit-violation termination accounting, not normal exit count. GPU counters are optional and are not collected in Job mode.

The unchanged gates are idle average CPU <=1%, resident private mean and sampled maximum <=350 MiB, and import sampled private peak <=600 MiB. A Job import result can only report memory evidence; import completion needs its separate ledger receipt. A `passed` idle result qualifies these resource metrics, not the entire application, release build, startup, or native interactions.

For an existing process, `measure-resources.ps1 -RootProcessId <id>` remains available. It records stable creation identities and observed descendants but cannot prove all exited child CPU. It can never return an idle `gate: passed`; its `thresholdGate` reports only the sampled arithmetic.

Controlled tests, requiring only the installed rustc and PowerShell:

```powershell
./scripts/test-resource-metrics.ps1
# Reuse unchanged native CPU evidence when checking only new protocol/gate cases:
./scripts/test-resource-metrics.ps1 -ReuseJobMetricsPath <passed-fixture-job.samples.jsonl>
```

The CPU fixture explicitly fails its prerequisite if scheduler contention makes its 220 ms CPU child live longer than one sample interval; that failure is retained. It does not enlarge the sampling window or reduce its CPU target. The known passed native receipt is `output/performance/observer-test-776f2f4fe49d4565ad0f99c753c7719b/job.samples.jsonl`: child wall time 0.5581725 seconds, child and Job CPU delta both 2,343,750 x100 ns, observation 10.0094932 seconds. Its new protocol/containment/failed-launch checks were subsequently run against this retained native input. This is composite script evidence, not a fresh full application resource run. Earlier failed prerequisites remain in the `5dac030d...` and `49ee2275...` directories.
