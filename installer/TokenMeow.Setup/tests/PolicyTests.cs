using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using TokenMeow.Setup;

internal static class PolicyTests
{
    private static int passed;
    private static void Check(bool condition, string name) { if (!condition) throw new Exception(name); }
    private static void Test(string name, Action action) { action(); passed++; Console.WriteLine("PASS " + name); }
    private static void Reject(Action action) { try { action(); } catch (InvalidOperationException) { return; } throw new Exception("expected rejection"); }
    private static InstallOptions Options() { return new InstallOptions { InstallDirectory = @"C:\Apps\Token Meow", CreateShortcuts = false, LaunchAfterInstall = true }; }

    private sealed class FakeLease : IEngineLease
    {
        private readonly Fake owner;
        internal FakeLease(Fake owner) { this.owner = owner; }
        public string Path { get { return @"C:\Private\engine.exe"; } }
        public void Dispose() { owner.Log.Add("engine-dispose"); }
    }
    private sealed class FakeGuard : IDisposable
    {
        private readonly Fake owner;
        internal FakeGuard(Fake owner) { this.owner = owner; }
        public void Dispose() { owner.Log.Add("guard-dispose"); }
    }
    private sealed class FakeProof : IApplicationProof
    {
        public string Directory { get; private set; }
        public string Executable { get { return System.IO.Path.Combine(Directory, "token-meow.exe"); } }
        internal FakeProof(string directory) { Directory = directory; }
        public void Dispose() { }
    }
    private sealed class Fake : IInstallerPlatform
    {
        internal readonly List<string> Log = new List<string>();
        internal bool Running, FailVerification, FailLaunch;
        internal int ExitCode;
        internal string Chosen, Arguments;
        public InstallerEnvironment ReadEnvironment() { Log.Add("environment"); return new InstallerEnvironment { DefaultDirectory = Options().InstallDirectory }; }
        public string ValidateTarget(string directory) { Log.Add("validate"); Chosen = directory; return InstallPolicy.NormalizeDirectory(directory, null); }
        public IDisposable GuardTarget(string directory) { Log.Add("guard"); return new FakeGuard(this); }
        public bool TargetIsRunning(string executable) { Log.Add("running-check"); return Running; }
        public IEngineLease ExtractEngine() { Log.Add("extract"); return new FakeLease(this); }
        public IInstallTransaction BeginTransaction(string directory) { Log.Add("transaction"); return new FakeTransaction(this); }
        public int RunEngine(IEngineLease engine, string arguments) { Log.Add("run"); Arguments = arguments; return ExitCode; }
        public IApplicationProof VerifyInstalled(string directory) { Log.Add("verify"); if (FailVerification) throw new InvalidOperationException("fake installed SHA mismatch"); return new FakeProof(directory); }
        public void Launch(IApplicationProof application) { Log.Add("launch"); if (FailLaunch) throw new InvalidOperationException("fake launch failure"); }
    }
    private sealed class FakeTransaction : IInstallTransaction
    {
        private readonly Fake owner;
        internal FakeTransaction(Fake owner) { this.owner=owner; }
        public void Commit() { owner.Log.Add("commit"); }
        public void Rollback() { owner.Log.Add("rollback"); }
        public void Dispose() { }
    }

    private static int Main()
    {
        try { return Run(); }
        catch (Exception error)
        {
            Console.Error.WriteLine("FAIL " + error.GetType().FullName + ": " + error.Message);
            return 1;
        }
    }

    private static int Run()
    {
        Test("only declared preview arguments are accepted", delegate {
            string mode;
            Check(Program.ParseArguments(new string[0], out mode) && mode == null, "normal welcome");
            foreach (string expected in new string[] { "welcome", "progress", "success", "error" })
            {
                string arg = expected == "welcome" ? "--preview" : "--preview=" + expected;
                Check(Program.ParseArguments(new string[] { arg }, out mode) && mode == expected, "explicit preview mode");
            }
            Check(!Program.ParseArguments(new string[] { "/S" }, out mode), "no engine flag passthrough");
            Check(!Program.ParseArguments(new string[] { "--preview", "/R" }, out mode), "no extra switches");
        });
        Test("local absolute spaces and unicode preserve NSIS D argument", delegate {
            string path = @"D:\应用\Token Meow";
            Check(InstallPolicy.EngineArguments(path, true) == "/S /D=" + path, "absolute D last");
            Check(InstallPolicy.EngineArguments(path, false) == "/S /NS /D=" + path, "NS is one combined switch");
            Check(InstallPolicy.EngineArguments(@"C:\Users\O'Connor\Token Meow", true) == @"/S /D=C:\Users\O'Connor\Token Meow", "apostrophe is a legal path without a shell");
        });
        Test("root UNC device relative quotes controls traversal rejected", delegate {
            foreach (string value in new string[] { @"C:\", @"\\host\share\app", @"\\?\C:\app", "C:app", ".\\app", "C:\\app\"bad", "C:\\app\nbad", @"C:\one\..\two", @"C:\one.\two", @"C:\NUL\app", @"C:\one:stream\app" })
                Reject(delegate { InstallPolicy.NormalizeDirectory(value, null); });
        });
        Test("source overlap rejected without rejecting lexical neighbor", delegate {
            string[] roots = new string[] { @"D:\Source\Installer", @"C:\Windows" };
            foreach (string value in new string[] { @"D:\Source\Installer", @"D:\Source\Installer\nested", @"D:\Source", @"C:\Windows\app" })
                Reject(delegate { InstallPolicy.NormalizeDirectory(value, roots); });
            Check(InstallPolicy.NormalizeDirectory(@"D:\Source\Installer-neighbor", roots) == @"D:\Source\Installer-neighbor", "neighbor allowed");
        });
        Test("upgrade uses old exact directory case insensitive and rejects relocation", delegate {
            InstallPolicy.RequireSameUpgradeDirectory(@"D:\Apps\TokenMeow", @"d:\apps\tokenmeow\");
            Reject(delegate { InstallPolicy.RequireSameUpgradeDirectory(@"D:\Elsewhere\TokenMeow", @"D:\Apps\TokenMeow"); });
        });
        Test("Codex and product data metadata roots cannot become install targets", delegate {
            var roots = InstallPolicy.DataDirectories(@"C:\Users\Fixture", @"C:\Users\Fixture\AppData\Local", @"C:\Users\Fixture\AppData\Roaming", @"D:\SyntheticCodex", @"D:\SyntheticData");
            foreach (string value in new string[] { @"C:\Users\Fixture\.codex", @"C:\Users\Fixture\.codex\sessions", @"C:\Users\Fixture\AppData\Local\local.tokenmeow.desktop", @"C:\Users\Fixture\AppData\Roaming\local.tokenmeow.desktop", @"D:\SyntheticCodex", @"D:\SyntheticData\nested" })
                Reject(delegate { InstallPolicy.NormalizeDirectory(value, roots); });
            Check(InstallPolicy.NormalizeDirectory(@"C:\Users\Fixture\AppData\Local\TokenMeow", roots) == @"C:\Users\Fixture\AppData\Local\TokenMeow", "normal app location remains valid");
        });
        Test("preview has no platform or launch effects", delegate {
            Fake platform = new Fake();
            using (InstallerService service = new InstallerService(true, platform))
            {
                Check(service.ReadEnvironment().Preview, "preview environment");
                List<int> steps = new List<int>();
                InstallResult result = service.InstallAsync(Options(), delegate(InstallProgress p) { steps.Add(p.Step); Check(p.Title.StartsWith("预览", StringComparison.Ordinal), "marked preview"); }).GetAwaiter().GetResult();
                service.LaunchInstalled();
                Check(result.Preview && result.ExecutablePath == null && String.Join(",", steps) == "0,1,2", "preview result/stages");
                Check(platform.Log.Count == 0, "zero platform operations");
            }
        });
        Test("real success requires verify and launches only after explicit request", delegate {
            Fake platform = new Fake();
            using (InstallerService service = new InstallerService(false, platform))
            {
                InstallOptions options = Options();
                var task = service.InstallAsync(options, delegate(InstallProgress p) { platform.Log.Add("step" + p.Step); });
                options.InstallDirectory = @"C:\Mutated";
                InstallResult result = task.GetAwaiter().GetResult();
                Check(result.Preview == false && platform.Chosen == @"C:\Apps\Token Meow", "snapshot options");
                Check(String.Join(",", platform.Log) == "step0,validate,guard,running-check,extract,transaction,step1,run,step2,verify,commit,engine-dispose,guard-dispose", "real ordering");
                Check(platform.Arguments == @"/S /NS /D=C:\Apps\Token Meow", "fixed arguments");
                Check(!platform.Log.Contains("launch"), "no auto launch");
                service.LaunchInstalled();
                Check(platform.Log[platform.Log.Count - 1] == "launch", "explicit launch");
            }
        });
        Test("running target stops before extract or run", delegate {
            Fake platform = new Fake { Running = true };
            using (InstallerService service = new InstallerService(false, platform))
            {
                Reject(delegate { service.InstallAsync(Options(), null).GetAwaiter().GetResult(); });
                Check(String.Join(",", platform.Log) == "validate,guard,running-check,guard-dispose", "no engine after running");
                Reject(delegate { service.LaunchInstalled(); });
            }
        });
        Test("Restart Manager successful end preserves probe result", delegate {
            int ends = 0;
            Check(!InstallerNative.ProbeWithSessionEnd(delegate { return false; }, delegate { ends++; return 0; }), "stopped result");
            Check(ends == 1, "end exactly once");
            Check(InstallerNative.ProbeWithSessionEnd(delegate { return true; }, delegate { return 0; }), "running result");
        });
        Test("Restart Manager failed end rejects an otherwise successful probe", delegate {
            bool rejected = false;
            try { InstallerNative.ProbeWithSessionEnd(delegate { return false; }, delegate { return 121; }); }
            catch (System.ComponentModel.Win32Exception error) { rejected = error.NativeErrorCode == 121; }
            Check(rejected, "nonzero end must not report a stopped target");
        });
        Test("Restart Manager cleanup failures preserve the primary probe exception", delegate {
            Exception primary = new InvalidOperationException("original probe failure");
            foreach (bool cleanupThrows in new bool[] { false, true })
            {
                Exception caught = null;
                int ends = 0;
                try
                {
                    InstallerNative.ProbeWithSessionEnd(delegate { throw primary; }, delegate {
                        ends++;
                        if (cleanupThrows) throw new System.ComponentModel.Win32Exception(29);
                        return 29;
                    });
                }
                catch (Exception error) { caught = error; }
                Check(Object.ReferenceEquals(caught, primary) && ends == 1, "original exception identity retained after cleanup");
            }
        });
        Test("every nonzero engine gate refuses success and releases resources", delegate {
            foreach (int code in new int[] { 1, 2, 64, 65, 66, 67, 68, 69, -1 })
            {
                Fake platform = new Fake { ExitCode = code };
                using (InstallerService service = new InstallerService(false, platform))
                {
                    Reject(delegate { service.InstallAsync(Options(), null).GetAwaiter().GetResult(); });
                    Reject(delegate { service.LaunchInstalled(); });
                    Check(!platform.Log.Contains("verify") && !platform.Log.Contains("launch") && platform.Log.Contains("engine-dispose") && platform.Log.Contains("guard-dispose") && platform.Log.Contains("rollback") && !platform.Log.Contains("commit"), "failed engine gate restores before release");
                }
            }
        });
        Test("exit zero without installed proof never becomes success", delegate {
            Fake platform = new Fake { FailVerification = true };
            using (InstallerService service = new InstallerService(false, platform))
            {
                Reject(delegate { service.InstallAsync(Options(), null).GetAwaiter().GetResult(); });
                Reject(delegate { service.LaunchInstalled(); });
                Check(platform.Log.Contains("verify") && platform.Log.Contains("engine-dispose") && platform.Log.Contains("rollback") && !platform.Log.Contains("commit"), "failed verification rolls back");
            }
        });
        Test("launch failure keeps the independently verified installation fact", delegate {
            Fake platform = new Fake { FailLaunch = true };
            using (InstallerService service = new InstallerService(false, platform))
            {
                InstallResult result = service.InstallAsync(Options(), null).GetAwaiter().GetResult();
                Reject(delegate { service.LaunchInstalled(); });
                Check(result.Preview == false && result.ExecutablePath.EndsWith("token-meow.exe", StringComparison.Ordinal), "installed fact retained");
                platform.FailLaunch = false;
                service.LaunchInstalled();
                Check(platform.Log.FindAll(delegate(string x) { return x == "verify"; }).Count == 1, "no false reinstall after launch failure");
            }
        });
        Test("stream hash resets and rejects wrong embedded or installed bytes", delegate {
            byte[] bytes = System.Text.Encoding.UTF8.GetBytes("private non-executable fixture");
            string expected;
            using (SHA256 sha = SHA256.Create()) expected = BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "");
            using (MemoryStream stream = new MemoryStream(bytes))
            {
                WindowsInstallerPlatform.RequireHash(stream, expected, "mismatch");
                Check(stream.Position == 0, "stream reset");
                Reject(delegate { WindowsInstallerPlatform.RequireHash(stream, new string('0', 64), "mismatch"); });
            }
        });
        Test("native private directory and nofollow held-read reject write delete", delegate {
            string directory = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "private-fixture-" + Guid.NewGuid().ToString("N"));
            InstallerNative.CreatePrivateDirectory(directory);
            string file = Path.Combine(directory, "not-an-engine.txt");
            try
            {
                using (FileStream output = new FileStream(file, FileMode.CreateNew, FileAccess.Write, FileShare.None)) output.WriteByte(42);
                using (InstallerNative.DirectoryGuards guard = InstallerNative.GuardExistingDirectories(directory))
                using (FileStream held = InstallerNative.OpenPlainFileRead(file))
                {
                    Check(held.ReadByte() == 42, "held correct file");
                    bool blockedWrite = false, blockedDelete = false;
                    try { using (FileStream writer = new FileStream(file, FileMode.Open, FileAccess.Write, FileShare.ReadWrite | FileShare.Delete)) { } } catch (IOException) { blockedWrite = true; }
                    try { File.Delete(file); } catch (IOException) { blockedDelete = true; }
                    Check(blockedWrite && blockedDelete, "share read blocks mutation");
                }
                File.WriteAllText(file, "writable only after handle close");
            }
            finally { if (File.Exists(file)) File.Delete(file); Directory.Delete(directory, false); }
        });
        Test("missing target segments are created and final directory retained before engine", delegate {
            string parent = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "missing-target-fixture-" + Guid.NewGuid().ToString("N"));
            string middle = Path.Combine(parent, "first");
            string target = Path.Combine(middle, "target");
            string child = Path.Combine(target, "non-executable.txt");
            try
            {
                using (InstallerNative.DirectoryGuards guard = InstallerNative.EnsureAndGuardTarget(target))
                {
                    Check(Directory.Exists(target), "final target exists before engine");
                    File.WriteAllText(child, "owned non-executable fixture");
                    bool blocked = false;
                    try { Directory.Move(target, target + "-moved"); } catch (IOException) { blocked = true; }
                    Check(blocked && Directory.Exists(target), "final target rename blocked while retained");
                    File.Delete(child);
                }
            }
            finally { if (File.Exists(child)) File.Delete(child); if (Directory.Exists(target)) Directory.Delete(target, false); if (Directory.Exists(middle)) Directory.Delete(middle, false); if (Directory.Exists(parent)) Directory.Delete(parent, false); }
        });
        Test("transaction restores old bytes and removes only declared new files", delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"rollback-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"), storage=Path.Combine(parent,"backup");
            Directory.CreateDirectory(target); File.WriteAllText(Path.Combine(target,"token-meow.exe"),"old exe");
            File.WriteAllText(Path.Combine(target,"user-note.txt"),"preserved");
            using(var tx=new InstallTransaction(target,storage,new string[]{"token-meow.exe","new.dll"},null,null,false,true))
            {
                File.WriteAllText(Path.Combine(target,"token-meow.exe"),"broken upgrade");
                File.WriteAllText(Path.Combine(target,"new.dll"),"new");
                File.WriteAllText(Path.Combine(target,"concurrent-note.txt"),"outside payload");
                tx.Rollback();
            }
            Check(File.ReadAllText(Path.Combine(target,"token-meow.exe"))=="old exe","old exe restored");
            Check(!File.Exists(Path.Combine(target,"new.dll")),"declared introduced file removed");
            Check(File.ReadAllText(Path.Combine(target,"user-note.txt"))=="preserved" && File.Exists(Path.Combine(target,"concurrent-note.txt")),"user files retained");
            Check(!Directory.Exists(storage),"completed rollback backup cleaned");
        });
        Test("pending journal recovers after installer interruption before next attempt", delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"recover-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"), storage=Path.Combine(parent,"backup");
            Directory.CreateDirectory(target);File.WriteAllText(Path.Combine(target,"token-meow.exe"),"original");
            var interrupted=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,true);
            File.WriteAllText(Path.Combine(target,"token-meow.exe"),"interrupted");interrupted.Dispose();
            using(var retry=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,true))
            { Check(File.ReadAllText(Path.Combine(target,"token-meow.exe"))=="original","pending attempt restored before new snapshot");retry.Commit(); }
        });
        Test("corrupt backup refuses recovery and preserves evidence", delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"corrupt-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"), storage=Path.Combine(parent,"backup");
            Directory.CreateDirectory(target);File.WriteAllText(Path.Combine(target,"token-meow.exe"),"original");
            var interrupted=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,true);
            File.WriteAllText(Path.Combine(target,"token-meow.exe"),"changed");
            File.WriteAllText(Directory.GetFiles(storage,"*.bak")[0],"corruption");
            Reject(delegate { interrupted.Rollback(); });
            Check(File.Exists(Path.Combine(storage,"journal.xml")) && File.ReadAllText(Path.Combine(target,"token-meow.exe"))=="changed","no destructive recovery after bad hash");
        });
        Test("fresh install rejects nonempty destinations and traversal payloads", delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"fresh-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"), storage=Path.Combine(parent,"backup");
            Directory.CreateDirectory(target);File.WriteAllText(Path.Combine(target,"user.txt"),"keep");
            Reject(delegate { new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,false); });
            foreach(string name in new string[]{"../escape","C:/escape","/root","one//two","one/../two","one/other:stream"}) Reject(delegate { InstallTransaction.ValidRelative(name); });
            Check(File.ReadAllText(Path.Combine(target,"user.txt"))=="keep","existing files unchanged");
        });
        Test("rollback restores typed registry values only inside owned test namespace", delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"registry-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"), storage=Path.Combine(parent,"backup");Directory.CreateDirectory(target);
            string key=@"Software\TokenMeow.LifecycleTests\"+Guid.NewGuid().ToString("N");
            using(var hive=Microsoft.Win32.RegistryKey.OpenBaseKey(Microsoft.Win32.RegistryHive.CurrentUser,Microsoft.Win32.RegistryView.Registry64))
            {
                using(var install=hive.CreateSubKey(key+@"\Install")) {install.SetValue("DisplayVersion","0.1.0");install.SetValue("Flags",17,Microsoft.Win32.RegistryValueKind.DWord);install.SetValue("Blob",new byte[]{1,2,3},Microsoft.Win32.RegistryValueKind.Binary);}
                using(var run=hive.CreateSubKey(key+@"\Run")) {run.SetValue("TokenMeow","%LOCALAPPDATA%\\old.exe",Microsoft.Win32.RegistryValueKind.ExpandString);run.SetValue("OtherApp","untouched");}
                using(var tx=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,true,true,key))
                {
                    using(var install=hive.CreateSubKey(key+@"\Install")) {install.SetValue("DisplayVersion","0.1.1");install.SetValue("NewValue","new");}
                    using(var run=hive.CreateSubKey(key+@"\Run")) run.SetValue("TokenMeow","new.exe");
                    tx.Rollback();
                }
                using(var install=hive.OpenSubKey(key+@"\Install")) {Check((string)install.GetValue("DisplayVersion")=="0.1.0" && (int)install.GetValue("Flags")==17 && ((byte[])install.GetValue("Blob"))[2]==3 && install.GetValue("NewValue")==null,"old typed install state restored");}
                using(var run=hive.OpenSubKey(key+@"\Run")) {Check(run.GetValueKind("TokenMeow")==Microsoft.Win32.RegistryValueKind.ExpandString && (string)run.GetValue("TokenMeow",null,Microsoft.Win32.RegistryValueOptions.DoNotExpandEnvironmentNames)=="%LOCALAPPDATA%\\old.exe" && (string)run.GetValue("OtherApp")=="untouched","only named Run value restored");}
                // Keep the exact private registry fixture as evidence; never touch product registrations.
            }
        });
        Test("failed fresh installation removes introduced empty directories and permits retry",delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"fresh-retry-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"),storage=Path.Combine(parent,"backup");Directory.CreateDirectory(target);
            using(var tx=new InstallTransaction(target,storage,new string[]{"token-meow.exe","licenses/a/license.txt"},null,null,false,false))
            {Directory.CreateDirectory(Path.Combine(target,"licenses/a"));File.WriteAllText(Path.Combine(target,"licenses/a/license.txt"),"partial");tx.Rollback();}
            Check(Directory.GetFileSystemEntries(target).Length==0,"fresh target restored to empty");
            using(var retry=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,false))retry.Commit();
        });
        Test("startup recovery entry point restores a pending filesystem journal",delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"startup-recovery-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"),storage=Path.Combine(parent,"backup");Directory.CreateDirectory(target);
            File.WriteAllText(Path.Combine(target,"token-meow.exe"),"old");
            using(var tx=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,true))File.WriteAllText(Path.Combine(target,"token-meow.exe"),"partial");
            InstallTransaction.RecoverInterrupted(target,storage,null,null);
            Check(File.ReadAllText(Path.Combine(target,"token-meow.exe"))=="old"&&!Directory.Exists(storage),"startup restored and closed journal");
        });
        Test("incomplete commit marker never suppresses interrupted installation recovery",delegate {
            string parent=Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"commit-recovery-"+Guid.NewGuid().ToString("N"));
            string target=Path.Combine(parent,"app"),storage=Path.Combine(parent,"backup");Directory.CreateDirectory(target);
            File.WriteAllText(Path.Combine(target,"token-meow.exe"),"old");
            using(var tx=new InstallTransaction(target,storage,new string[]{"token-meow.exe"},null,null,false,true))File.WriteAllText(Path.Combine(target,"token-meow.exe"),"partial");
            File.WriteAllBytes(Path.Combine(storage,"committed"),new byte[0]);File.WriteAllBytes(Path.Combine(storage,"commit.pending"),new byte[]{1});
            InstallTransaction.RecoverInterrupted(target,storage,null,null);
            Check(File.ReadAllText(Path.Combine(target,"token-meow.exe"))=="old"&&!Directory.Exists(storage),"torn marker restored old state and closed journal");
        });
        Console.WriteLine("RESULT " + passed + " passed; no production installation, product registry, engine or App execution");
        return 0;
    }
}
