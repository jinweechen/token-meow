using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Xml;
using System.Xml.Serialization;
using Microsoft.Win32;

namespace TokenMeow.Setup
{
    internal interface IInstallTransaction : IDisposable { void Commit(); void Rollback(); }

    // The journal is durable before the engine is allowed to write. Recovery
    // restores program files and installer state only, never the user's ledger.
    public sealed class InstallJournal
    {
        public int Format = 1;
        public string Target;
        public List<InstallFileSnapshot> Files = new List<InstallFileSnapshot>();
        public List<string> Destinations = new List<string>();
        public bool DirectoriesCaptured;
        public List<string> Directories = new List<string>();
        public List<InstallRegistrySnapshot> Registry = new List<InstallRegistrySnapshot>();
    }
    public sealed class InstallFileSnapshot { public string Name, Backup, Sha256; }
    public sealed class InstallRegistrySnapshot
    {
        public string Key, Name, Data;
        public int Kind;
        public bool Exists;
    }

    internal sealed class InstallTransaction : IInstallTransaction
    {
        internal const string UninstallKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\TokenMeow";
        internal const string PreferencesKey = @"Software\tokenmeow\TokenMeow";
        internal const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
        private readonly string target, storage, desktop, menu;
        private readonly bool useRegistry;
        private readonly string uninstallKey=UninstallKey, preferencesKey=PreferencesKey, runKey=RunKey;
        private InstallJournal journal;

        internal InstallTransaction(string target, string storage, IEnumerable<string> installedFiles,
            string desktop, string menu, bool useRegistry, bool existingInstallation, string registrySandbox = null)
        {
            this.target = Path.GetFullPath(target).TrimEnd('\\');
            this.storage = Path.GetFullPath(storage);
            this.desktop = desktop; this.menu = menu; this.useRegistry = useRegistry;
            if (registrySandbox != null)
            {
                if (!registrySandbox.StartsWith(@"Software\TokenMeow.LifecycleTests\",StringComparison.Ordinal) || registrySandbox.Contains("..")) throw new InvalidOperationException("Invalid registry test namespace.");
                uninstallKey=registrySandbox+@"\Install";preferencesKey=registrySandbox+@"\Preferences";runKey=registrySandbox+@"\Run";
            }
            using (InstallerNative.GuardExistingDirectories(Path.GetDirectoryName(this.storage)))
            {
                if (Directory.Exists(this.storage))
                {
                    using (InstallerNative.GuardExistingDirectories(this.storage)) Recover();
                    Cleanup();
                }
            }
            if (!existingInstallation && Directory.GetFileSystemEntries(this.target).Length != 0)
                throw new InvalidOperationException("首次安装请选择空目录，不能覆盖已有文件。已有安装请使用其原目录。");
            InstallerNative.CreatePrivateDirectory(this.storage);
            journal = new InstallJournal { Target = this.target };
            foreach (string dir in EnumeratePlainDirectories(this.target)) journal.Directories.Add(dir.Substring(this.target.Length+1).Replace('\\','/'));
            journal.DirectoriesCaptured=true;
            var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (string name in installedFiles) names.Add("app/" + ValidRelative(name));
            names.Add("app/uninstall.exe");
            if (!String.IsNullOrEmpty(desktop)) names.Add("desktop/TokenMeow.lnk");
            if (!String.IsNullOrEmpty(menu)) names.Add("menu/TokenMeow.lnk");
            long bytes = 0;
            foreach (string file in EnumeratePlainFiles(this.target))
            {
                string name = "app/" + file.Substring(this.target.Length + 1).Replace('\\', '/');
                names.Add(name);
                bytes += new FileInfo(file).Length;
                if (names.Count > 8192 || bytes > 2L * 1024 * 1024 * 1024)
                    throw new InvalidOperationException("现有安装目录过大，已停止更新并保留全部文件。");
            }
            foreach (string name in names)
            {
                string path = Resolve(name);
                journal.Destinations.Add(name);
                if (!File.Exists(path)) continue;
                string backup = journal.Files.Count.ToString("D6") + ".bak";
                using (var source = InstallerNative.OpenPlainFileRead(path))
                using (var output = new FileStream(Path.Combine(this.storage, backup), FileMode.CreateNew, FileAccess.Write, FileShare.None))
                { source.CopyTo(output); output.Flush(true); }
                journal.Files.Add(new InstallFileSnapshot { Name = name, Backup = backup, Sha256 = Digest(Path.Combine(this.storage, backup)) });
            }
            if (useRegistry)
            {
                CaptureKey(uninstallKey); CaptureKey(preferencesKey);
                CaptureValue(runKey, "TokenMeow");
            }
            using (var output = new FileStream(Path.Combine(this.storage, "journal.xml"), FileMode.CreateNew, FileAccess.Write, FileShare.None))
            { new XmlSerializer(typeof(InstallJournal)).Serialize(output, journal); output.Flush(true); }
        }

        internal static string StorageName(string target)
        {
            using (var sha = SHA256.Create())
                return BitConverter.ToString(sha.ComputeHash(Encoding.UTF8.GetBytes(Path.GetFullPath(target).TrimEnd('\\').ToUpperInvariant()))).Replace("-", "");
        }

        private InstallTransaction(string target, string storage, string desktop, string menu)
        { this.target=target; this.storage=storage; this.desktop=desktop; this.menu=menu; this.useRegistry=true; }

        internal static string RecoveryTarget(string storage)
        {
            using (InstallerNative.GuardExistingDirectories(storage))
            {
                string path=Path.Combine(storage,"journal.xml");
                if (!File.Exists(path)) return null;
                InstallerNative.RequirePlainFile(path);
                using (var reader=XmlReader.Create(path,new XmlReaderSettings { DtdProcessing=DtdProcessing.Prohibit,XmlResolver=null,MaxCharactersInDocument=8*1024*1024 }))
                    return ((InstallJournal)new XmlSerializer(typeof(InstallJournal)).Deserialize(reader)).Target;
            }
        }

        internal static void RecoverInterrupted(string target, string storage, string desktop, string menu)
        {
            var transaction=new InstallTransaction(target,storage,desktop,menu);
            using (InstallerNative.GuardExistingDirectories(target))
            using (InstallerNative.GuardExistingDirectories(storage)) transaction.Recover();
            transaction.Cleanup();
        }

        internal static string ValidRelative(string name)
        {
            if (String.IsNullOrEmpty(name) || name.IndexOf(':') >= 0 || name.StartsWith("/") || name.StartsWith("\\"))
                throw new InvalidOperationException("安装文件路径无效。");
            name = name.Replace('\\', '/');
            foreach (string part in name.Split('/'))
                if (part.Length == 0 || part == "." || part == ".." || part.EndsWith(".") || part != part.Trim() || part.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0)
                    throw new InvalidOperationException("安装文件路径无效。");
            return name;
        }

        private string Resolve(string name)
        {
            name = ValidRelative(name);
            string root;
            if (name.StartsWith("app/", StringComparison.Ordinal)) root = target;
            else if (name == "desktop/TokenMeow.lnk") root = desktop;
            else if (name == "menu/TokenMeow.lnk") root = menu;
            else throw new InvalidOperationException("恢复记录包含不受支持的文件位置。");
            if (String.IsNullOrEmpty(root)) throw new InvalidOperationException("无法确定快捷方式目录。");
            return Path.Combine(root, name.Substring(name.IndexOf('/') + 1).Replace('/', '\\'));
        }

        private static IEnumerable<string> EnumeratePlainFiles(string root)
        {
            using (InstallerNative.GuardExistingDirectories(root))
            {
                foreach (string entry in Directory.GetFileSystemEntries(root))
                {
                    var attrs = File.GetAttributes(entry);
                    if ((attrs & FileAttributes.ReparsePoint) != 0) throw new InvalidOperationException("安装目录含链接，无法安全备份或恢复。");
                    if ((attrs & FileAttributes.Directory) != 0)
                        foreach (string child in EnumeratePlainFiles(entry)) yield return child;
                    else { InstallerNative.RequirePlainFile(entry); yield return entry; }
                }
            }
        }

        private static IEnumerable<string> EnumeratePlainDirectories(string root)
        {
            using (InstallerNative.GuardExistingDirectories(root))
                foreach (string entry in Directory.GetDirectories(root))
                {
                    if ((File.GetAttributes(entry)&FileAttributes.ReparsePoint)!=0) throw new InvalidOperationException("安装目录含链接，不能安全恢复。");
                    yield return entry;
                    foreach (string child in EnumeratePlainDirectories(entry)) yield return child;
                }
        }

        private static string Digest(string path)
        { using (var stream = InstallerNative.OpenPlainFileRead(path)) using (var sha = SHA256.Create()) return BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", ""); }

        private void Recover()
        {
            string path = Path.Combine(storage, "journal.xml");
            if (!File.Exists(path) || IsCommitted()) return;
            InstallerNative.RequirePlainFile(path);
            using (var input = XmlReader.Create(path, new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null, MaxCharactersInDocument = 8 * 1024 * 1024 }))
                journal = (InstallJournal)new XmlSerializer(typeof(InstallJournal)).Deserialize(input);
            if (journal.Format != 1 || !InstallPolicy.SameDirectory(journal.Target, target) || journal.Destinations.Count > 8192)
                throw new InvalidOperationException("安装恢复记录不匹配；已保留备份，不能继续覆盖安装。");
            Restore();
            WriteCommitMarker();
        }

        private void Restore()
        {
            var backups = new Dictionary<string, InstallFileSnapshot>(StringComparer.OrdinalIgnoreCase);
            foreach (var file in journal.Files)
            {
                Resolve(file.Name);
                if (!System.Text.RegularExpressions.Regex.IsMatch(file.Backup ?? "", @"^\d{6}\.bak$") || backups.ContainsKey(file.Name))
                    throw new InvalidOperationException("安装恢复记录损坏，已保留备份。");
                if (Digest(Path.Combine(storage, file.Backup)) != file.Sha256)
                    throw new InvalidOperationException("安装备份校验失败，已保留全部恢复资料。");
                backups.Add(file.Name, file);
            }
            // Validate every destination and registry entry before any recovery writes.
            foreach (string name in journal.Destinations) Resolve(name);
            foreach (string dir in journal.Directories) ValidRelative(dir);
            foreach (var value in journal.Registry) ValidateRegistry(value);
            foreach (string name in journal.Destinations)
            {
                string path = Resolve(name);
                using (InstallerNative.GuardExistingDirectories(Path.GetDirectoryName(path)))
                {
                    InstallFileSnapshot file;
                    if (backups.TryGetValue(name, out file))
                    {
                        Directory.CreateDirectory(Path.GetDirectoryName(path));
                        using (InstallerNative.GuardExistingDirectories(Path.GetDirectoryName(path)))
                        {
                            if (File.Exists(path)) InstallerNative.RequirePlainFile(path);
                            using (var source=InstallerNative.OpenPlainFileRead(Path.Combine(storage,file.Backup)))
                            using (var output=new FileStream(path,FileMode.Create,FileAccess.Write,FileShare.None))
                            { source.CopyTo(output); output.Flush(true); }
                            if (Digest(path) != file.Sha256) throw new IOException("恢复文件校验失败。");
                        }
                    }
                    else if (File.Exists(path)) { InstallerNative.RequirePlainFile(path); File.Delete(path); }
                }
            }
            if (journal.DirectoriesCaptured)
            {
                var original=new HashSet<string>(journal.Directories,StringComparer.OrdinalIgnoreCase);
                var introduced=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (string name in journal.Destinations)
                {
                    if (!name.StartsWith("app/",StringComparison.Ordinal)) continue;
                    string relative=name.Substring(4);
                    while(relative.LastIndexOf('/')>0)
                    { relative=relative.Substring(0,relative.LastIndexOf('/')); if(!original.Contains(relative)) introduced.Add(relative); }
                }
                var ordered=new List<string>(introduced);ordered.Sort(delegate(string a,string b){return b.Length.CompareTo(a.Length);});
                foreach(string relative in ordered)
                {
                    string dir=Path.Combine(target,ValidRelative(relative).Replace('/','\\'));
                    using(InstallerNative.GuardExistingDirectories(Path.GetDirectoryName(dir)))
                    {
                        if(!Directory.Exists(dir)) continue;
                        using(InstallerNative.GuardExistingDirectories(dir))
                            if(Directory.GetFileSystemEntries(dir).Length!=0) continue;
                        Directory.Delete(dir,false);
                    }
                }
            }
            if (useRegistry) RestoreRegistry();
        }

        public void Commit()
        {
            WriteCommitMarker();
            try { Cleanup(); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
        private bool IsCommitted()
        {
            string path=Path.Combine(storage,"committed");
            if(!File.Exists(path)) return false;
            using(var input=InstallerNative.OpenPlainFileRead(path)) return input.Length==1 && input.ReadByte()==1;
        }
        private void WriteCommitMarker()
        {
            if(IsCommitted()) return;
            string committed=Path.Combine(storage,"committed"),pending=Path.Combine(storage,"commit.pending");
            foreach(string path in new string[]{committed,pending})
                if(File.Exists(path)){InstallerNative.RequirePlainFile(path);File.Delete(path);}
            using(var marker=new FileStream(pending,FileMode.CreateNew,FileAccess.Write,FileShare.None)) {marker.WriteByte(1);marker.Flush(true);}
            File.Move(pending,committed);
        }
        public void Rollback()
        {
            try { Restore(); Commit(); }
            catch (Exception error) { throw new InvalidOperationException("安装失败且自动恢复未完成。备份保留在 " + storage + "；请关闭应用后重新运行安装器恢复。" + error.Message, error); }
        }
        public void Dispose() { /* Never discard a pending durable recovery journal. */ }

        private void Cleanup()
        {
            // Exact allowlist; never recursively delete a computed directory.
            foreach (string file in Directory.GetFiles(storage))
            {
                string name = Path.GetFileName(file);
                if (name != "journal.xml" && name != "committed" && name != "commit.pending" && !System.Text.RegularExpressions.Regex.IsMatch(name, @"^\d{6}\.bak$"))
                    throw new InvalidOperationException("恢复目录含未知文件，已保留。");
                InstallerNative.RequirePlainFile(file);
            }
            // Remove the journal last so interrupted cleanup cannot resemble a new transaction.
            foreach (string file in Directory.GetFiles(storage)) if (Path.GetFileName(file) != "journal.xml" && Path.GetFileName(file) != "committed") File.Delete(file);
            File.Delete(Path.Combine(storage, "journal.xml")); File.Delete(Path.Combine(storage, "committed"));
            Directory.Delete(storage, false);
        }

        private void CaptureKey(string path)
        {
            using (var hive = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Registry64))
            using (var key = hive.OpenSubKey(path))
            {
                journal.Registry.Add(new InstallRegistrySnapshot { Key = path, Name = null, Exists = key != null });
                if (key == null) return;
                if (key.GetSubKeyNames().Length != 0) throw new InvalidOperationException("安装记录含未知子项，已停止更新。");
                foreach (string name in key.GetValueNames()) CaptureValue(path, name);
            }
        }
        private void CaptureValue(string path, string name)
        {
            using (var hive = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Registry64))
            using (var key = hive.OpenSubKey(path))
            {
                object value = key == null ? null : key.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);
                var row = new InstallRegistrySnapshot { Key = path, Name = name, Exists = value != null };
                if (value != null)
                {
                    row.Kind = (int)key.GetValueKind(name);
                    using (var output = new MemoryStream())
                    {
                        new XmlSerializer(value.GetType()).Serialize(output, value);
                        row.Data = Convert.ToBase64String(output.ToArray());
                    }
                }
                journal.Registry.Add(row);
            }
        }
        private void ValidateRegistry(InstallRegistrySnapshot row)
        {
            if (row.Key != uninstallKey && row.Key != preferencesKey && !(row.Key == runKey && row.Name == "TokenMeow"))
                throw new InvalidOperationException("恢复记录包含不受支持的注册表位置。");
        }
        private void RestoreRegistry()
        {
            using (var hive = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Registry64))
            {
                foreach (var row in journal.Registry)
                {
                    ValidateRegistry(row);
                    if (row.Name == null)
                    {
                        // Clear only the exact owned key; CaptureKey refused subkeys.
                        using (var current = hive.OpenSubKey(row.Key, true))
                        {
                            if (current != null && current.GetSubKeyNames().Length != 0) throw new IOException("安装记录出现未知子项，已保留备份。");
                            if (current != null) foreach (string name in current.GetValueNames()) current.DeleteValue(name, false);
                        }
                        if (!row.Exists) hive.DeleteSubKey(row.Key, false);
                        continue;
                    }
                    using (var key = hive.CreateSubKey(row.Key))
                    {
                        if (!row.Exists) { key.DeleteValue(row.Name, false); continue; }
                        var kind = (RegistryValueKind)row.Kind;
                        Type type = kind == RegistryValueKind.Binary || kind == RegistryValueKind.None ? typeof(byte[]) : kind == RegistryValueKind.MultiString ? typeof(string[]) : kind == RegistryValueKind.DWord ? typeof(int) : kind == RegistryValueKind.QWord ? typeof(long) : typeof(string);
                        using (var input = new MemoryStream(Convert.FromBase64String(row.Data)))
                        using (var reader = XmlReader.Create(input, new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null }))
                            key.SetValue(row.Name, new XmlSerializer(type).Deserialize(reader), kind);
                    }
                }
            }
        }
    }
}
