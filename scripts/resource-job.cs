// Acceptance observer only. This code is not linked into the application.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace TokenMeow.Acceptance {
    public sealed class JobSample {
        public string kind = "sample";
        public string jobId;
        public double elapsedSeconds;
        public string capturedUtc;
        public long totalUser100ns, totalKernel100ns;
        public uint activeProcesses, totalProcesses, terminatedProcesses;
        public long privateBytes, workingSetBytes, handleCount;
        public ulong jobLifetimePeakCommitBytes;
        public bool rootExited;
        public List<object> processes = new List<object>();
        public List<string> measurementErrors = new List<string>();
    }

    public sealed class ResourceJob : IDisposable {
        const uint Suspended = 0x4, NoWindow = 0x08000000, QueryRead = 0x00100410;
        readonly IntPtr job;
        readonly ProcessInformation root;
        readonly Stopwatch clock = new Stopwatch();
        public readonly string JobId = Guid.NewGuid().ToString("D");
        public readonly long RootCreationFileTime;
        public readonly bool ObserverOutsideJob;
        bool resumed, disposed;

        public int RootProcessId { get { return (int)root.processId; } }
        public static int LogicalProcessors { get { return checked((int)GetActiveProcessorCount(0xffff)); } }

        public ResourceJob(string executable, string[] arguments, string workingDirectory) {
            job = CreateJobObjectW(IntPtr.Zero, null);
            if (job == IntPtr.Zero) throw Error("CreateJobObject");
            var startup = new StartupInfo(); startup.cb = (uint)Marshal.SizeOf(typeof(StartupInfo));
            var command = new StringBuilder(Quote(executable));
            foreach (var argument in arguments ?? new string[0]) command.Append(" ").Append(Quote(argument));
            if (!CreateProcessW(executable, command, IntPtr.Zero, IntPtr.Zero, false,
                Suspended | NoWindow, IntPtr.Zero, workingDirectory, ref startup, out root)) {
                var error = Error("CreateProcess"); CloseHandle(job); throw error;
            }
            if (!AssignProcessToJobObject(job, root.process)) {
                var error = Error("AssignProcessToJobObject");
                // Only the new, still suspended fixture/application is terminated on launch failure.
                TerminateProcess(root.process, 1); CloseHandle(root.thread); CloseHandle(root.process); CloseHandle(job);
                throw error;
            }
            bool observerMember;
            if (!IsProcessInJob(GetCurrentProcess(), job, out observerMember) || observerMember) {
                var error = new InvalidOperationException("The observer must remain outside its application Job.");
                TerminateProcess(root.process, 1); CloseHandle(root.thread); CloseHandle(root.process); CloseHandle(job); throw error;
            }
            ObserverOutsideJob = true;
            long created, exited, kernel, user;
            if (!GetProcessTimes(root.process, out created, out exited, out kernel, out user)) {
                var error = Error("GetProcessTimes(root)"); TerminateProcess(root.process, 1);
                CloseHandle(root.thread); CloseHandle(root.process); CloseHandle(job); throw error;
            }
            RootCreationFileTime = created;
            // No BREAKAWAY_OK, SILENT_BREAKAWAY_OK, or KILL_ON_JOB_CLOSE flags are set.
            // The observer is outside this Job; closing it leaves the application running.
            clock.Start();
        }

        public void Resume() {
            if (resumed) throw new InvalidOperationException("Root was already resumed.");
            if (ResumeThread(root.thread) == 0xffffffff) throw Error("ResumeThread");
            resumed = true;
        }

        public JobSample Sample() {
            var sample = new JobSample(); sample.jobId = JobId;
            var accounting = new BasicAndIo();
            if (!QueryAccounting(job, 8, out accounting, (uint)Marshal.SizeOf(typeof(BasicAndIo)), IntPtr.Zero))
                throw Error("QueryInformationJobObject(accounting)");
            sample.elapsedSeconds = clock.Elapsed.TotalSeconds;
            sample.capturedUtc = DateTime.UtcNow.ToString("o");
            sample.totalUser100ns = accounting.basic.totalUserTime;
            sample.totalKernel100ns = accounting.basic.totalKernelTime;
            sample.activeProcesses = accounting.basic.activeProcesses;
            sample.totalProcesses = accounting.basic.totalProcesses;
            // This is limit-violation termination accounting, NOT a normal-exit counter.
            sample.terminatedProcesses = accounting.basic.totalTerminatedProcesses;
            sample.rootExited = WaitForSingleObject(root.process, 0) == 0;
            var limits = new ExtendedLimit();
            if (QueryLimits(job, 9, out limits, (uint)Marshal.SizeOf(typeof(ExtendedLimit)), IntPtr.Zero))
                sample.jobLifetimePeakCommitBytes = limits.peakJobMemoryUsed.ToUInt64();
            else sample.measurementErrors.Add("job-lifetime-peak:" + Marshal.GetLastWin32Error());
            foreach (int processId in ProcessIds()) {
                IntPtr handle = OpenProcess(QueryRead, false, processId);
                if (handle == IntPtr.Zero) {
                    // ERROR_INVALID_PARAMETER means the process has already exited.
                    int error = Marshal.GetLastWin32Error();
                    if (error != 87) sample.measurementErrors.Add("open-process:" + processId + ":" + error);
                    continue;
                }
                try {
                    bool member;
                    if (!IsProcessInJob(handle, job, out member)) {
                        sample.measurementErrors.Add("job-membership:" + processId + ":" + Marshal.GetLastWin32Error()); continue;
                    }
                    // A reused PID from the preceding Job list never acquires ownership.
                    if (!member || WaitForSingleObject(handle, 0) == 0) continue;
                    long created, exited, kernel, user;
                    if (!GetProcessTimes(handle, out created, out exited, out kernel, out user)) {
                        sample.measurementErrors.Add("process-time:" + processId + ":" + Marshal.GetLastWin32Error()); continue;
                    }
                    var memory = new ProcessMemory(); memory.cb = (uint)Marshal.SizeOf(typeof(ProcessMemory));
                    uint handles;
                    if (!GetProcessMemoryInfo(handle, ref memory, memory.cb) || !GetProcessHandleCount(handle, out handles)) {
                        // Exited processes contribute no current memory; their CPU remains in Job accounting.
                        if (WaitForSingleObject(handle, 0) != 0)
                            sample.measurementErrors.Add("process-memory:" + processId + ":" + Marshal.GetLastWin32Error());
                        continue;
                    }
                    long privateBytes = checked((long)memory.privateUsage.ToUInt64());
                    long workingBytes = checked((long)memory.workingSetSize.ToUInt64());
                    sample.privateBytes += privateBytes; sample.workingSetBytes += workingBytes; sample.handleCount += handles;
                    sample.processes.Add(new { id = processId, creationFileTime = created,
                        privateBytes = privateBytes, workingSetBytes = workingBytes, handles = handles });
                } finally { CloseHandle(handle); }
            }
            return sample;
        }

        int[] ProcessIds() {
            int capacity = 64;
            for (int attempt = 0; attempt < 8; attempt++, capacity *= 2) {
                int bytes = checked(8 + capacity * IntPtr.Size);
                IntPtr buffer = Marshal.AllocHGlobal(bytes);
                try {
                    if (!QueryBuffer(job, 3, buffer, (uint)bytes, IntPtr.Zero)) {
                        int error = Marshal.GetLastWin32Error();
                        if (error == 234) continue;
                        throw new Win32Exception(error, "QueryInformationJobObject(process-list)");
                    }
                    int count = Marshal.ReadInt32(buffer, 4);
                    if (count < 0 || count > capacity) throw new InvalidOperationException("Invalid Job process count.");
                    var result = new int[count];
                    for (int i = 0; i < count; i++) result[i] = checked((int)Marshal.ReadIntPtr(buffer, 8 + i * IntPtr.Size).ToInt64());
                    return result;
                } finally { Marshal.FreeHGlobal(buffer); }
            }
            throw new InvalidOperationException("Job process list exceeded the bounded observer capacity.");
        }

        public void Dispose() {
            if (disposed) return; disposed = true;
            // Failure before Resume must not strand a suspended process. A running
            // application is deliberately left alone when observation ends/fails.
            if (!resumed) TerminateProcess(root.process, 1);
            CloseHandle(root.thread); CloseHandle(root.process); CloseHandle(job);
        }
        static Exception Error(string operation) { return new Win32Exception(Marshal.GetLastWin32Error(), operation); }
        static string Quote(string value) {
            var result = new StringBuilder("\""); int slashes = 0;
            foreach (char c in value) {
                if (c == '\\') { slashes++; continue; }
                if (c == '"') { result.Append('\\', slashes * 2 + 1).Append(c); slashes = 0; continue; }
                result.Append('\\', slashes).Append(c); slashes = 0;
            }
            return result.Append('\\', slashes * 2).Append('"').ToString();
        }

        [StructLayout(LayoutKind.Sequential)] struct StartupInfo {
            public uint cb; public IntPtr reserved, desktop, title; public uint x,y,xSize,ySize,xChars,yChars,fill,flags;
            public ushort showWindow, reserved2; public IntPtr reservedPointer, input, output, error;
        }
        [StructLayout(LayoutKind.Sequential)] struct ProcessInformation { public IntPtr process,thread; public uint processId,threadId; }
        [StructLayout(LayoutKind.Sequential)] struct BasicAccounting {
            public long totalUserTime,totalKernelTime,periodUserTime,periodKernelTime;
            public uint pageFaultCount,totalProcesses,activeProcesses,totalTerminatedProcesses;
        }
        [StructLayout(LayoutKind.Sequential)] struct IoCounters { public ulong readOps,writeOps,otherOps,readBytes,writeBytes,otherBytes; }
        [StructLayout(LayoutKind.Sequential)] struct BasicAndIo { public BasicAccounting basic; public IoCounters io; }
        [StructLayout(LayoutKind.Sequential)] struct BasicLimit {
            public long processTime,jobTime; public uint flags; public UIntPtr minWorking,maxWorking;
            public uint activeLimit; public UIntPtr affinity; public uint priority,scheduling;
        }
        [StructLayout(LayoutKind.Sequential)] struct ExtendedLimit {
            public BasicLimit basic; public IoCounters io; public UIntPtr processMemory,jobMemory,peakProcessMemory,peakJobMemoryUsed;
        }
        [StructLayout(LayoutKind.Sequential)] struct ProcessMemory {
            public uint cb,pageFaults; public UIntPtr peakWorkingSetSize,workingSetSize,peakPaged,paged,peakNonPaged,nonPaged,pageFile,peakPageFile,privateUsage;
        }
        [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] static extern IntPtr CreateJobObjectW(IntPtr security,string name);
        [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] static extern bool CreateProcessW(string app,StringBuilder command,IntPtr processAttributes,IntPtr threadAttributes,bool inherit,uint flags,IntPtr environment,string directory,ref StartupInfo startup,out ProcessInformation process);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
        [DllImport("kernel32.dll",SetLastError=true)] static extern uint ResumeThread(IntPtr thread);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint code);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool CloseHandle(IntPtr handle);
        [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,int processId);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetProcessTimes(IntPtr process,out long created,out long exited,out long kernel,out long user);
        [DllImport("kernel32.dll",SetLastError=true)] static extern uint WaitForSingleObject(IntPtr handle,uint timeout);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool IsProcessInJob(IntPtr process,IntPtr job,out bool result);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetProcessHandleCount(IntPtr process,out uint count);
        [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
        [DllImport("kernel32.dll")] static extern uint GetActiveProcessorCount(ushort group);
        [DllImport("psapi.dll",SetLastError=true)] static extern bool GetProcessMemoryInfo(IntPtr process,ref ProcessMemory counters,uint size);
        [DllImport("kernel32.dll",EntryPoint="QueryInformationJobObject",SetLastError=true)] static extern bool QueryAccounting(IntPtr job,int info,out BasicAndIo result,uint length,IntPtr returned);
        [DllImport("kernel32.dll",EntryPoint="QueryInformationJobObject",SetLastError=true)] static extern bool QueryLimits(IntPtr job,int info,out ExtendedLimit result,uint length,IntPtr returned);
        [DllImport("kernel32.dll",EntryPoint="QueryInformationJobObject",SetLastError=true)] static extern bool QueryBuffer(IntPtr job,int info,IntPtr result,uint length,IntPtr returned);
    }
}
