using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

namespace TokenMeow.Setup
{
    internal static class InstallerNative
    {
        [StructLayout(LayoutKind.Sequential)]
        private struct SecurityAttributes { public int Length; public IntPtr Descriptor; [MarshalAs(UnmanagedType.Bool)] public bool Inherit; }
        [StructLayout(LayoutKind.Sequential)]
        private struct FileInformation
        {
            public uint Attributes; public System.Runtime.InteropServices.ComTypes.FILETIME Creation, Access, Write;
            public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct UniqueProcess { public int ProcessId; public System.Runtime.InteropServices.ComTypes.FILETIME StartTime; }
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct RmProcessInfo
        {
            public UniqueProcess Process;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string AppName;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string ServiceName;
            public uint AppType, AppStatus, SessionId;
            [MarshalAs(UnmanagedType.Bool)] public bool Restartable;
        }

        internal sealed class DirectoryGuards : IDisposable
        {
            private readonly List<SafeFileHandle> handles = new List<SafeFileHandle>();
            internal void Add(string directory)
            {
                // GENERIC_READ participates in Windows sharing checks. A handle
                // opened only for READ_ATTRIBUTES does not retain rename safety.
                SafeFileHandle handle = CreateFile(directory, 0x80000000, 3, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
                if (handle.IsInvalid) { handle.Dispose(); throw Error("无法保护安装目录"); }
                try
                {
                    FileInformation info;
                    if (!GetFileInformationByHandle(handle, out info)) throw Error("无法读取安装目录身份");
                    if ((info.Attributes & 0x400) != 0 || (info.Attributes & 0x10) == 0)
                        throw new InvalidOperationException("安装路径不能包含链接、联接点或重解析目录。");
                    handles.Add(handle);
                }
                catch { handle.Dispose(); throw; }
            }
            public void Dispose() { foreach (SafeFileHandle handle in handles) handle.Dispose(); handles.Clear(); }
        }

        internal static DirectoryGuards GuardExistingDirectories(string directory)
        {
            return GuardDirectories(directory, false);
        }

        internal static DirectoryGuards EnsureAndGuardTarget(string directory)
        {
            return GuardDirectories(directory, true);
        }

        private static DirectoryGuards GuardDirectories(string directory, bool createMissing)
        {
            DirectoryGuards guards = new DirectoryGuards();
            try
            {
                string root = Path.GetPathRoot(directory);
                string current = root;
                guards.Add(current);
                foreach (string part in directory.Substring(root.Length).Split(new char[] { '\\' }, StringSplitOptions.RemoveEmptyEntries))
                {
                    current = Path.Combine(current, part);
                    uint attrs = GetFileAttributes(current);
                    if (attrs == 0xffffffff)
                    {
                        int error = Marshal.GetLastWin32Error();
                        if (error == 2 || error == 3)
                        {
                            if (!createMissing) break;
                            // Each parent is already held without share-delete.
                            // CreateDirectoryW rejects an unexpected existing name;
                            // never adopt a raced-in link or mutate existing ACLs.
                            CreatePrivateDirectory(current);
                            guards.Add(current);
                            continue;
                        }
                        throw new Win32Exception(error, "无法确认安装路径，已停止安装。");
                    }
                    if ((attrs & 0x400) != 0 || (attrs & 0x10) == 0)
                        throw new InvalidOperationException("安装路径不能包含链接、联接点或普通文件。");
                    guards.Add(current);
                }
                return guards;
            }
            catch { guards.Dispose(); throw; }
        }

        internal static void RequirePlainFile(string path)
        {
            FileAttributes attrs = File.GetAttributes(path);
            if ((attrs & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                throw new InvalidOperationException("应用或安装引擎必须是普通文件，不能使用链接。");
        }

        internal static FileStream OpenPlainFileRead(string path)
        {
            // Open the directory entry itself; never follow a replaced symlink.
            SafeFileHandle handle = CreateFile(path, 0x80000000, 1, IntPtr.Zero, 3, 0x08200000, IntPtr.Zero);
            if (handle.IsInvalid) { handle.Dispose(); throw Error("无法持有已校验文件"); }
            try
            {
                FileInformation info;
                if (!GetFileInformationByHandle(handle, out info)) throw Error("无法读取文件身份");
                if ((info.Attributes & (0x400 | 0x10)) != 0)
                    throw new InvalidOperationException("应用或安装引擎必须是普通文件，不能使用链接。");
                return new FileStream(handle, FileAccess.Read, 65536, false);
            }
            catch { handle.Dispose(); throw; }
        }

        internal static void CreatePrivateDirectory(string path)
        {
            DirectorySecurity security = new DirectorySecurity();
            security.SetAccessRuleProtection(true, false);
            SecurityIdentifier user;
            using (WindowsIdentity identity = WindowsIdentity.GetCurrent()) user = identity.User;
            if (user == null) throw new InvalidOperationException("无法确认当前用户身份，已停止安装。");
            security.SetOwner(user);
            security.AddAccessRule(new FileSystemAccessRule(user, FileSystemRights.FullControl,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
            security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null), FileSystemRights.FullControl,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
            byte[] descriptor = security.GetSecurityDescriptorBinaryForm();
            GCHandle pin = GCHandle.Alloc(descriptor, GCHandleType.Pinned);
            try
            {
                SecurityAttributes attrs = new SecurityAttributes { Length = Marshal.SizeOf(typeof(SecurityAttributes)), Descriptor = pin.AddrOfPinnedObject(), Inherit = false };
                if (!CreateDirectory(path, ref attrs)) throw Error("无法创建私有安装缓存目录");
            }
            finally { pin.Free(); }
        }

        internal static bool TargetIsRunning(string executable)
        {
            if (!File.Exists(executable)) return false;
            uint session;
            System.Text.StringBuilder key = new System.Text.StringBuilder(33);
            int code = RmStartSession(out session, 0, key);
            if (code != 0) throw new Win32Exception(code, "无法确认应用运行状态，已停止安装。");
            return ProbeWithSessionEnd(delegate
            {
                code = RmRegisterResources(session, 1, new string[] { executable }, 0, null, 0, null);
                if (code != 0) throw new Win32Exception(code, "无法确认应用运行状态，已停止安装。");
                uint needed = 0, count = 0, reason;
                code = RmGetList(session, out needed, ref count, null, out reason);
                if (code == 234)
                {
                    if (needed == 0) throw new InvalidOperationException("应用运行状态不明确，已停止安装。");
                    return true;
                }
                if (code != 0) throw new Win32Exception(code, "无法确认应用运行状态，已停止安装。");
                return needed > 0;
            }, delegate { return RmEndSession(session); });
        }

        internal static bool ProbeWithSessionEnd(Func<bool> probe, Func<int> endSession)
        {
            bool probeFailed = false;
            try { return probe(); }
            catch { probeFailed = true; throw; }
            finally
            {
                if (probeFailed)
                {
                    // Always attempt cleanup, while retaining the primary probe
                    // failure and its original stack rather than hiding it.
                    try { endSession(); } catch { }
                }
                else
                {
                    int ended = endSession();
                    if (ended != 0)
                        throw new Win32Exception(ended, "无法结束应用运行状态检查，已停止安装。");
                }
            }
        }

        private static Win32Exception Error(string action) { return new Win32Exception(Marshal.GetLastWin32Error(), action); }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateFileW")]
        private static extern SafeFileHandle CreateFile(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
        [DllImport("kernel32.dll", SetLastError = true)] private static extern bool GetFileInformationByHandle(SafeFileHandle file, out FileInformation info);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "GetFileAttributesW")]
        private static extern uint GetFileAttributes(string path);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateDirectoryW")]
        private static extern bool CreateDirectory(string path, ref SecurityAttributes security);
        [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)] private static extern int RmStartSession(out uint session, int flags, System.Text.StringBuilder key);
        [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)] private static extern int RmRegisterResources(uint session, uint fileCount, string[] files, uint applicationCount, UniqueProcess[] applications, uint serviceCount, string[] services);
        [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)] private static extern int RmGetList(uint session, out uint needed, ref uint count, [In, Out] RmProcessInfo[] processes, out uint reason);
        [DllImport("rstrtmgr.dll")] private static extern int RmEndSession(uint session);
    }
}
