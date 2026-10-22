using System;
using System.Diagnostics;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32;

namespace TokenMeow.Setup
{
    internal interface IEngineLease : IDisposable { string Path { get; } }
    internal interface IApplicationProof : IDisposable { string Directory { get; } string Executable { get; } }
    internal interface IInstallerPlatform
    {
        InstallerEnvironment ReadEnvironment();
        string ValidateTarget(string directory);
        IDisposable GuardTarget(string directory);
        bool TargetIsRunning(string executable);
        IEngineLease ExtractEngine();
        IInstallTransaction BeginTransaction(string directory);
        int RunEngine(IEngineLease engine, string arguments);
        IApplicationProof VerifyInstalled(string directory);
        void Launch(IApplicationProof application);
    }

    internal sealed class InstallerService : IDisposable
    {
        private readonly bool preview;
        private readonly IInstallerPlatform platform;
        private readonly object gate = new object();
        private IApplicationProof completed;
        private bool busy, disposed;

        internal InstallerService(bool preview, IInstallerPlatform platform)
        {
            this.preview = preview;
            this.platform = platform;
            if (!preview && platform == null) throw new ArgumentNullException("platform");
        }

        internal InstallerEnvironment ReadEnvironment()
        {
            if (preview) return new InstallerEnvironment {
                DefaultDirectory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "TokenMeow"),
                InstalledVersion = null, WebView2Present = false, Preview = true
            };
            return platform.ReadEnvironment();
        }

        internal Task<InstallResult> InstallAsync(InstallOptions options, Action<InstallProgress> progress)
        {
            if (options == null) throw new ArgumentNullException("options");
            InstallOptions snapshot = new InstallOptions { InstallDirectory = options.InstallDirectory, CreateShortcuts = options.CreateShortcuts, LaunchAfterInstall = options.LaunchAfterInstall };
            lock (gate)
            {
                if (disposed) throw new ObjectDisposedException("InstallerService");
                if (busy) throw new InvalidOperationException("安装正在进行，请等待完成。");
                if (completed != null) { completed.Dispose(); completed = null; }
                busy = true;
            }
            return Task.Run(delegate
            {
                try { return Install(snapshot, progress); }
                finally { lock (gate) { busy = false; } }
            });
        }

        private InstallResult Install(InstallOptions options, Action<InstallProgress> progress)
        {
            if (preview)
            {
                Report(progress, 0, "预览 · 校验安装环境", "仅模拟界面；不会释放引擎、写入注册表或安装应用。");
                Thread.Sleep(350);
                Report(progress, 1, "预览 · 正在安装 Token 喵", "模拟安装阶段，没有安装进程或实际文件变更。");
                Thread.Sleep(350);
                Report(progress, 2, "预览 · 验证安装结果", "仅展示完成页面；点击打开也不会启动应用。");
                Thread.Sleep(350);
                return new InstallResult { InstallDirectory = options.InstallDirectory, ExecutablePath = null, LaunchAfterInstall = options.LaunchAfterInstall, Preview = true };
            }
            Report(progress, 0, "校验安装环境", "检查安装目录与应用运行状态，并校验内嵌安装引擎。");
            string target = platform.ValidateTarget(options.InstallDirectory);
            using (IDisposable guard = platform.GuardTarget(target))
            {
                if (platform.TargetIsRunning(Path.Combine(target, "token-meow.exe")))
                    throw new InvalidOperationException(InstallPolicy.EngineError(65));
                using (IEngineLease engine = platform.ExtractEngine())
                using (IInstallTransaction transaction = platform.BeginTransaction(target))
                {
                    try
                    {
                    Report(progress, 1, "正在安装 Token 喵", "安装引擎正在处理应用和运行时。缺少 WebView2 时需要联网下载，请等待完成。");
                    int code = platform.RunEngine(engine, InstallPolicy.EngineArguments(target, options.CreateShortcuts));
                    if (code != 0) throw new InvalidOperationException(InstallPolicy.EngineError(code));
                    Report(progress, 2, "验证安装结果", "核验已安装应用的 SHA-256 与当前用户安装记录。");
                    IApplicationProof proof = platform.VerifyInstalled(target);
                    if (proof == null) throw new InvalidOperationException("安装结果未通过核验。");
                    // Release the proof before rollback if committing the durable marker fails.
                    try { transaction.Commit(); } catch { proof.Dispose(); throw; }
                    lock (gate)
                    {
                        if (completed != null) completed.Dispose();
                        completed = proof;
                    }
                    return new InstallResult { InstallDirectory = proof.Directory, ExecutablePath = proof.Executable, LaunchAfterInstall = options.LaunchAfterInstall, Preview = false };
                    }
                    catch (Exception failure)
                    {
                        try { transaction.Rollback(); }
                        catch (Exception recovery) { throw new InvalidOperationException(failure.Message + "\n" + recovery.Message, recovery); }
                        throw new InvalidOperationException(failure.Message + "\n本次安装写入已撤回，原有程序和安装记录已恢复。", failure);
                    }
                }
            }
        }

        internal void LaunchInstalled()
        {
            lock (gate)
            {
                if (disposed) throw new ObjectDisposedException("InstallerService");
                if (preview) return;
                if (busy || completed == null) throw new InvalidOperationException("尚无已通过核验的安装结果。");
                platform.Launch(completed);
            }
        }

        public void Dispose()
        {
            lock (gate)
            {
                // Real UI blocks closing while busy. A programmatic close must not
                // dispose resources underneath the running engine or terminate it.
                if (busy) return;
                disposed = true;
                if (completed != null) { completed.Dispose(); completed = null; }
            }
        }

        private static void Report(Action<InstallProgress> callback, int step, string title, string detail)
        {
            if (callback != null) callback(new InstallProgress { Step = step, Title = title, Detail = detail });
        }
    }

    internal sealed class WindowsInstallerPlatform : IInstallerPlatform
    {
        private const string ProductKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\TokenMeow";
        private const string WebViewKey = @"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

        private sealed class Registration { internal string Directory, Version; }
        private sealed class EngineLease : IEngineLease
        {
            private readonly FileStream held;
            private readonly InstallerNative.DirectoryGuards guards;
            public string Path { get; private set; }
            internal EngineLease(string path, FileStream held, InstallerNative.DirectoryGuards guards)
            { Path = path; this.held = held; this.guards = guards; }
            public void Dispose()
            {
                try
                {
                    held.Dispose();
                    // Keep directory identity held while deleting our one file.
                    // The empty private cache is intentionally retained; never
                    // delete a path after releasing its directory identity.
                    try { File.Delete(Path); }
                    catch (IOException) { }
                    catch (UnauthorizedAccessException) { }
                }
                finally { guards.Dispose(); }
            }
        }
        private sealed class ApplicationProof : IApplicationProof
        {
            private readonly FileStream held;
            private readonly InstallerNative.DirectoryGuards guards;
            public string Directory { get; private set; }
            public string Executable { get; private set; }
            internal ApplicationProof(string directory, string executable, FileStream held, InstallerNative.DirectoryGuards guards)
            { Directory = directory; Executable = executable; this.held = held; this.guards = guards; }
            public void Dispose() { try { held.Dispose(); } finally { guards.Dispose(); } }
        }

        public InstallerEnvironment ReadEnvironment()
        {
            RecoverPendingInstallations();
            Registration installed = ReadRegistration();
            return new InstallerEnvironment {
                DefaultDirectory = installed == null ? Path.Combine(LocalData(), "TokenMeow") : installed.Directory,
                InstalledVersion = installed == null ? null : installed.Version,
                WebView2Present = HasWebView2(), Preview = false
            };
        }

        private void RecoverPendingInstallations()
        {
            string parent=Path.Combine(LocalData(),"TokenMeow.Setup","transactions");
            using (InstallerNative.GuardExistingDirectories(parent))
            {
                if (!Directory.Exists(parent)) return;
                foreach (string storage in Directory.GetDirectories(parent))
                {
                    if (!System.Text.RegularExpressions.Regex.IsMatch(Path.GetFileName(storage),"^[A-F0-9]{64}$")) continue;
                    string target=InstallTransaction.RecoveryTarget(storage);
                    if (target==null) continue; // Snapshot preparation ended before any engine writes.
                    target=InstallPolicy.NormalizeDirectory(target,ForbiddenDirectories());
                    if (Path.GetFileName(storage)!=InstallTransaction.StorageName(target)) throw new InvalidOperationException("安装恢复目录不匹配，已保留备份。");
                    if (InstallerNative.TargetIsRunning(Path.Combine(target,"token-meow.exe"))) throw new InvalidOperationException("上次安装未完成，请先退出 Token 喵，再重新运行安装器恢复。");
                    InstallTransaction.RecoverInterrupted(target,storage,Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),Environment.GetFolderPath(Environment.SpecialFolder.Programs));
                }
            }
        }

        public string ValidateTarget(string directory)
        {
            string target = InstallPolicy.NormalizeDirectory(directory, ForbiddenDirectories());
            Registration existing = ReadRegistration();
            InstallPolicy.RequireSameUpgradeDirectory(target, existing == null ? null : existing.Directory);
            string profile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
            if (!String.IsNullOrEmpty(profile) && InstallPolicy.SameDirectory(target, profile))
                throw new InvalidOperationException("请选择用户目录下独立的应用文件夹。");
            return target;
        }

        public IDisposable GuardTarget(string directory) { return InstallerNative.EnsureAndGuardTarget(directory); }
        public bool TargetIsRunning(string executable) { return InstallerNative.TargetIsRunning(executable); }

        public IInstallTransaction BeginTransaction(string directory)
        {
            string parent = Path.Combine(LocalData(), "TokenMeow.Setup", "transactions");
            using (InstallerNative.EnsureAndGuardTarget(parent))
                return new InstallTransaction(directory, Path.Combine(parent, InstallTransaction.StorageName(directory)),
                    BuildInfo.InstalledFiles, Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
                    Environment.GetFolderPath(Environment.SpecialFolder.Programs), true, ReadRegistration() != null);
        }

        public IEngineLease ExtractEngine()
        {
            string directory = Path.Combine(LocalData(), "TokenMeow.Setup-" + Guid.NewGuid().ToString("N"));
            string path = Path.Combine(directory, "engine.exe");
            FileStream held = null;
            InstallerNative.DirectoryGuards guards = InstallerNative.GuardExistingDirectories(LocalData());
            bool privateGuardHeld = false;
            try
            {
                InstallerNative.CreatePrivateDirectory(directory);
                guards.Add(directory);
                privateGuardHeld = true;
                using (Stream resource = Assembly.GetExecutingAssembly().GetManifestResourceStream(BuildInfo.EngineResourceName))
                {
                    if (resource == null) throw new InvalidOperationException("安装包缺少内嵌安装引擎，请重新获取完整安装包。");
                    using (FileStream output = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, FileOptions.SequentialScan))
                    { resource.CopyTo(output, 65536); output.Flush(true); }
                }
                held = InstallerNative.OpenPlainFileRead(path);
                RequireHash(held, BuildInfo.EngineSha256, "内嵌安装引擎校验失败，未执行安装。");
                EngineLease lease = new EngineLease(path, held, guards);
                held = null; guards = null;
                return lease;
            }
            finally
            {
                try { if (held != null) held.Dispose(); }
                finally
                {
                    if (guards != null)
                    {
                        try
                        {
                            try { if (privateGuardHeld) File.Delete(path); }
                            catch (IOException) { }
                            catch (UnauthorizedAccessException) { }
                        }
                        finally { guards.Dispose(); }
                    }
                }
            }
        }

        public int RunEngine(IEngineLease engine, string arguments)
        {
            ProcessStartInfo info = new ProcessStartInfo {
                FileName = engine.Path, Arguments = arguments,
                WorkingDirectory = Path.GetDirectoryName(engine.Path),
                UseShellExecute = false, CreateNoWindow = true
            };
            // The same Process retains the native handle from Start until exit;
            // never reopen by PID and never Kill an engine or an existing App.
            using (Process process = new Process())
            {
                process.StartInfo = info;
                if (!process.Start()) throw new InvalidOperationException("无法启动安装引擎。");
                IntPtr exactHandle = process.Handle;
                if (exactHandle == IntPtr.Zero) throw new InvalidOperationException("无法持有安装进程身份。");
                process.WaitForExit();
                return process.ExitCode;
            }
        }

        public IApplicationProof VerifyInstalled(string directory)
        {
            InstallerNative.DirectoryGuards guards = InstallerNative.GuardExistingDirectories(directory);
            FileStream held = null;
            try
            {
                string executable = Path.Combine(directory, "token-meow.exe");
                held = InstallerNative.OpenPlainFileRead(executable);
                RequireHash(held, BuildInfo.AppSha256, "安装后的应用校验失败。未将本次安装标记为成功。");
                Registration registration = ReadRegistration();
                if (registration == null || !InstallPolicy.SameDirectory(registration.Directory, directory) || registration.Version != BuildInfo.ProductVersion)
                    throw new InvalidOperationException("应用文件已写入，但当前用户安装记录未通过核验。请重试安装。");
                string uninstaller = Path.Combine(directory, "uninstall.exe");
                InstallerNative.RequirePlainFile(uninstaller);
                ApplicationProof proof = new ApplicationProof(directory, executable, held, guards);
                held = null; guards = null;
                return proof;
            }
            finally { if (held != null) held.Dispose(); if (guards != null) guards.Dispose(); }
        }

        public void Launch(IApplicationProof application)
        {
            // The verified EXE and its directory remain held while CreateProcess
            // opens the image. Launch failure does not revoke the installed fact.
            ProcessStartInfo info = new ProcessStartInfo { FileName = application.Executable, WorkingDirectory = application.Directory, UseShellExecute = false };
            using (Process process = Process.Start(info))
                if (process == null) throw new InvalidOperationException("无法打开已安装的 Token 喵。安装结果仍然有效，请稍后从应用快捷方式打开。");
        }

        private static Registration ReadRegistration()
        {
            using (RegistryKey hive = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Registry64))
            using (RegistryKey key = hive.OpenSubKey(ProductKey, false))
            {
                if (key == null) return null;
                string directory = ReadString(key, "InstallLocation");
                if (directory.Length >= 2 && directory[0] == '\"' && directory[directory.Length - 1] == '\"') directory = directory.Substring(1, directory.Length - 2);
                directory = InstallPolicy.NormalizeDirectory(directory, ForbiddenDirectories());
                if (ReadString(key, "MainBinaryName") != "token-meow.exe")
                    throw new InvalidOperationException("现有应用的安装身份不符，已保留原安装。");
                string uninstall = ReadString(key, "UninstallString");
                if (!String.Equals(uninstall, "\"" + Path.Combine(directory, "uninstall.exe") + "\"", StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("现有安装不支持本安装器的原位更新，已保留原安装。");
                return new Registration { Directory = directory, Version = ReadString(key, "DisplayVersion") };
            }
        }

        private static string ReadString(RegistryKey key, string name)
        {
            if (key.GetValueKind(name) != RegistryValueKind.String)
                throw new InvalidOperationException("安装记录包含不支持的数据类型，已停止安装。");
            string value = key.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames) as string;
            if (String.IsNullOrEmpty(value)) throw new InvalidOperationException("安装记录不完整，已停止安装。");
            return value;
        }

        private static string LocalData()
        {
            string path = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            if (String.IsNullOrEmpty(path) || !Directory.Exists(path)) throw new InvalidOperationException("无法找到当前用户的本地应用目录。");
            return path;
        }

        private static string[] ForbiddenDirectories()
        {
            List<string> roots = new List<string> { AppDomain.CurrentDomain.BaseDirectory,
                Environment.GetFolderPath(Environment.SpecialFolder.Windows),
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86) };
            roots.AddRange(InstallPolicy.DataDirectories(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                Environment.GetEnvironmentVariable("CODEX_HOME"),
                Environment.GetEnvironmentVariable("TOKEN_MEOW_DATA_DIR")));
            return roots.ToArray();
        }

        private static bool HasWebView2()
        {
            foreach (RegistryHive root in new RegistryHive[] { RegistryHive.CurrentUser, RegistryHive.LocalMachine })
                foreach (RegistryView view in new RegistryView[] { RegistryView.Registry32, RegistryView.Registry64 })
                    try
                    {
                        using (RegistryKey hive = RegistryKey.OpenBaseKey(root, view))
                        using (RegistryKey key = hive.OpenSubKey(WebViewKey, false))
                        {
                            string version = key == null ? null : key.GetValue("pv", null, RegistryValueOptions.DoNotExpandEnvironmentNames) as string;
                            Version parsed;
                            if (Version.TryParse(version, out parsed) && parsed > new Version(0, 0)) return true;
                        }
                    }
                    catch (System.Security.SecurityException) { }
                    catch (UnauthorizedAccessException) { }
            return false;
        }

        internal static void RequireHash(Stream stream, string expected, string message)
        {
            string actual;
            using (SHA256 sha = SHA256.Create()) actual = BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "");
            stream.Position = 0;
            if (String.IsNullOrEmpty(expected) || expected.Length != 64 || !String.Equals(actual, expected, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException(message);
        }
    }
}
