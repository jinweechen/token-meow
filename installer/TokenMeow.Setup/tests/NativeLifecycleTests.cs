using System;
using TokenMeow.Setup;

internal static class NativeLifecycleTests
{
    private sealed class FailAfterEngine : IInstallerPlatform
    {
        private readonly WindowsInstallerPlatform real = new WindowsInstallerPlatform();
        public InstallerEnvironment ReadEnvironment() { return real.ReadEnvironment(); }
        public string ValidateTarget(string path) { return real.ValidateTarget(path); }
        public IDisposable GuardTarget(string path) { return real.GuardTarget(path); }
        public bool TargetIsRunning(string path) { return real.TargetIsRunning(path); }
        public IEngineLease ExtractEngine() { return real.ExtractEngine(); }
        public IInstallTransaction BeginTransaction(string path) { return real.BeginTransaction(path); }
        public int RunEngine(IEngineLease engine, string args)
        {
            int code=real.RunEngine(engine,args);
            if(code!=0) return code;
            throw new InvalidOperationException("INJECTED_AFTER_REAL_ENGINE_SUCCESS");
        }
        public IApplicationProof VerifyInstalled(string path) { return real.VerifyInstalled(path); }
        public void Launch(IApplicationProof proof) { throw new InvalidOperationException("This native test must never launch the application."); }
    }
    private static int Main(string[] args)
    {
        try
        {
            if(args.Length!=2 || (args[0]!="install" && args[0]!="fail")) return 2;
            bool fail=args[0]=="fail";
            IInstallerPlatform platform=fail ? (IInstallerPlatform)new FailAfterEngine() : new WindowsInstallerPlatform();
            using(var service=new InstallerService(false,platform))
            {
                service.ReadEnvironment();
                try
                {
                    var result=service.InstallAsync(new InstallOptions {InstallDirectory=args[1],CreateShortcuts=false,LaunchAfterInstall=false},null).GetAwaiter().GetResult();
                    if(fail) throw new Exception("Injected failure was not observed");
                    if(result.Preview || result.LaunchAfterInstall || result.ExecutablePath==null) throw new Exception("No actual installed proof");
                    Console.WriteLine("REAL_SERVICE_AND_ENGINE_INSTALLED_AND_VERIFIED");
                }
                catch(InvalidOperationException error)
                {
                    if(!fail || !error.Message.Contains("INJECTED_AFTER_REAL_ENGINE_SUCCESS") || !error.Message.Contains("已撤回")) throw;
                    Console.WriteLine("REAL_ENGINE_WRITES_ROLLED_BACK_AFTER_INJECTED_FAILURE");
                }
            }
            return 0;
        }
        catch(Exception error) {Console.Error.WriteLine(error);return 1;}
    }
}
