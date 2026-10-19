using System;
using System.Threading.Tasks;
using System.Windows;

namespace TokenMeow.Setup
{
    internal static class Program
    {
        [STAThread]
        private static int Main(string[] args)
        {
            string previewMode;
            bool valid = ParseArguments(args, out previewMode);
            bool preview = previewMode != null || !valid;
            Application app = new Application { ShutdownMode = ShutdownMode.OnMainWindowClose };
            InstallerService service = new InstallerService(preview, preview ? null : new WindowsInstallerPlatform());
            InstallerEnvironment environment;
            string startupError = valid ? null : "启动参数无效。支持 --preview、--preview=progress、--preview=success 或 --preview=error。";
            try { environment = service.ReadEnvironment(); }
            catch (Exception ex)
            {
                environment = new InstallerEnvironment { DefaultDirectory = "", Preview = false };
                startupError = "无法读取安装环境：" + ex.Message;
            }
            InstallerWindow window = new InstallerWindow(environment);
            app.MainWindow = window;
            bool startupBlocked = startupError != null;
            bool closed = false;
            window.InstallRequested += async delegate(InstallOptions options)
            {
                if (startupBlocked) { window.ShowError(startupError); return; }
                try
                {
                    InstallResult result = await service.InstallAsync(options, delegate(InstallProgress progress) { if (!closed) window.ShowProgress(progress); });
                    if (!closed) window.ShowSuccess(result);
                }
                catch (Exception ex) { if (!closed) window.ShowError(ex.Message); }
            };
            window.LaunchRequested += async delegate
            {
                if (preview)
                {
                    window.ShowLaunchError("预览模式：安装与打开操作均未执行，可直接关闭此窗口。");
                    return;
                }
                try { await Task.Run(delegate { service.LaunchInstalled(); }); window.Close(); }
                catch (Exception ex) { window.ShowLaunchError(ex.Message); }
            };
            window.Closed += delegate { closed = true; service.Dispose(); };
            window.Loaded += delegate
            {
                if (startupBlocked) { window.ShowError(startupError); return; }
                if (previewMode == "progress") window.ShowProgress(new InstallProgress { Step = 1, Title = "预览 · 正在安装 Token 喵", Detail = "静态界面预览；没有安装进程、文件释放或系统变更，可直接关闭窗口。" });
                else if (previewMode == "success") window.ShowSuccess(new InstallResult { InstallDirectory = environment.DefaultDirectory, LaunchAfterInstall = true, Preview = true });
                else if (previewMode == "error") window.ShowError("预览 · 安装引擎返回示例错误。未执行安装，也未更改任何应用。");
            };
            return app.Run(window);
        }

        internal static bool ParseArguments(string[] args, out string previewMode)
        {
            previewMode = null;
            if (args.Length == 0) return true;
            if (args.Length != 1) return false;
            if (args[0] == "--preview") { previewMode = "welcome"; return true; }
            foreach (string mode in new string[] { "progress", "success", "error" })
                if (args[0] == "--preview=" + mode) { previewMode = mode; return true; }
            return false;
        }
    }
}
