using System;

namespace TokenMeow.Setup
{
    public sealed class InstallerEnvironment
    {
        public string DefaultDirectory { get; set; }
        public string InstalledVersion { get; set; }
        public bool WebView2Present { get; set; }
        public bool Preview { get; set; }
    }

    public sealed class InstallOptions
    {
        public string InstallDirectory { get; set; }
        public bool CreateShortcuts { get; set; }
        public bool LaunchAfterInstall { get; set; }
    }

    public sealed class InstallProgress
    {
        public int Step { get; set; }
        public string Title { get; set; }
        public string Detail { get; set; }
    }

    public sealed class InstallResult
    {
        public string InstallDirectory { get; set; }
        public string ExecutablePath { get; set; }
        public bool LaunchAfterInstall { get; set; }
        public bool Preview { get; set; }
    }
}
