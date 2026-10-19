using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

// Build-time file guards only. No process, registry or installer execution.
public sealed class TokenMeowInstallerBuildGuards : IDisposable
{
    private readonly Dictionary<string, SafeFileHandle> directories = new Dictionary<string, SafeFileHandle>(StringComparer.OrdinalIgnoreCase);
    [StructLayout(LayoutKind.Sequential)]
    private struct Info
    {
        public uint Attributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME Creation, Access, Write;
        public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
    }
    private static Info Inspect(SafeFileHandle handle)
    {
        Info info;
        if (!GetFileInformationByHandle(handle, out info)) throw new Win32Exception(Marshal.GetLastWin32Error());
        return info;
    }
    private void HoldDirectory(string path)
    {
        if (directories.ContainsKey(path)) return;
        SafeFileHandle handle = CreateFileW(path, 0x80000000, 3, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (handle.IsInvalid) { int error = Marshal.GetLastWin32Error(); handle.Dispose(); throw new Win32Exception(error, path); }
        try
        {
            Info info = Inspect(handle);
            if ((info.Attributes & 0x400) != 0 || (info.Attributes & 0x10) == 0) throw new IOException("Reparse or non-directory input ancestor: " + path);
            directories.Add(path, handle);
        }
        catch { handle.Dispose(); throw; }
    }
    public void HoldFolder(string path)
    {
        Stack<string> chain = new Stack<string>();
        while (!String.IsNullOrEmpty(path)) { chain.Push(path); path = Path.GetDirectoryName(path); }
        foreach (string item in chain) HoldDirectory(item);
    }
    public static void CreateNewDirectory(string path)
    {
        if (!CreateDirectoryW(path, IntPtr.Zero)) throw new Win32Exception(Marshal.GetLastWin32Error(), "New directory required: " + path);
    }
    public static Task<string> CapturePipe(Stream input, Stream output)
    {
        return Task.Run(delegate
        {
            string failure = null;
            byte[] buffer = new byte[65536];
            try
            {
                int read;
                while ((read = input.Read(buffer, 0, buffer.Length)) != 0)
                {
                    if (failure != null) continue;
                    try { output.Write(buffer, 0, read); }
                    catch (Exception error) { failure = error.GetType().Name + ": " + error.Message; }
                }
            }
            catch (Exception error)
            {
                if (failure == null) failure = error.GetType().Name + ": " + error.Message;
                input.Dispose(); // Signal a broken pipe, never leave a writer blocked.
            }
            return failure;
        });
    }
    public FileStream OpenRead(string path)
    {
        path = Path.GetFullPath(path);
        string directory = Path.GetDirectoryName(path);
        HoldFolder(directory);
        SafeFileHandle handle = CreateFileW(path, 0x80000000, 1, IntPtr.Zero, 3, 0x08200000, IntPtr.Zero);
        if (handle.IsInvalid) { int error = Marshal.GetLastWin32Error(); handle.Dispose(); throw new Win32Exception(error, path); }
        try
        {
            Info info = Inspect(handle);
            if ((info.Attributes & 0x410) != 0) throw new IOException("Expected an ordinary input file: " + path);
            return new FileStream(handle, FileAccess.Read, 65536, false);
        }
        catch { handle.Dispose(); throw; }
    }
    public static string Identity(FileStream stream)
    {
        Info info = Inspect(stream.SafeFileHandle);
        return info.Volume.ToString("X8") + ":" + info.IndexHigh.ToString("X8") + info.IndexLow.ToString("X8") + ":" + info.SizeHigh.ToString("X8") + info.SizeLow.ToString("X8") + ":" + info.Write.dwHighDateTime.ToString("X8") + info.Write.dwLowDateTime.ToString("X8");
    }
    public void Dispose() { foreach (SafeFileHandle handle in directories.Values) handle.Dispose(); directories.Clear(); }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle file, out Info info);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateDirectoryW(string path, IntPtr security);
}
