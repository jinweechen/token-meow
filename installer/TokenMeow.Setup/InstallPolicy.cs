using System;
using System.Collections.Generic;
using System.IO;

namespace TokenMeow.Setup
{
    internal static class InstallPolicy
    {
        internal static string NormalizeDirectory(string value, IEnumerable<string> forbiddenRoots)
        {
            if (String.IsNullOrWhiteSpace(value) || value != value.Trim())
                throw new InvalidOperationException("请选择完整的本地安装目录，路径两端不能包含空格。");
            foreach (char c in value)
                if (Char.IsControl(c) || c == '\"')
                    throw new InvalidOperationException("安装目录不能包含引号或控制字符。");
            value = value.Replace('/', '\\');
            if (value.Length < 4 || !Char.IsLetter(value[0]) || value[1] != ':' || value[2] != '\\')
                throw new InvalidOperationException("安装目录必须是本地磁盘的绝对路径，不支持网络或设备路径。");
            string[] parts = value.Substring(3).TrimEnd('\\').Split('\\');
            foreach (string part in parts)
            {
                if (part.Length == 0 || part == "." || part == ".." || part.EndsWith(".", StringComparison.Ordinal) || part != part.Trim())
                    throw new InvalidOperationException("安装目录包含不明确的路径片段。");
                if (part.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0)
                    throw new InvalidOperationException("安装目录包含 Windows 不支持的字符。");
                string stem = part.Split('.')[0].ToUpperInvariant();
                if (stem == "CON" || stem == "PRN" || stem == "AUX" || stem == "NUL" ||
                    (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.Ordinal) || stem.StartsWith("LPT", StringComparison.Ordinal)) && stem[3] >= '1' && stem[3] <= '9'))
                    throw new InvalidOperationException("安装目录包含保留的设备名称。");
            }
            string full = Path.GetFullPath(value).TrimEnd('\\');
            if (full.Length <= 3 || String.Equals(full, Path.GetPathRoot(full).TrimEnd('\\'), StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("不能安装到磁盘根目录。");
            if (forbiddenRoots != null)
                foreach (string forbidden in forbiddenRoots)
                    if (!String.IsNullOrEmpty(forbidden) && Overlap(full, Path.GetFullPath(forbidden).TrimEnd('\\')))
                        throw new InvalidOperationException("请选择独立的应用目录，不能覆盖系统目录或安装器所在目录。");
            return full;
        }

        internal static bool SameDirectory(string a, string b)
        {
            return String.Equals(Path.GetFullPath(a).TrimEnd('\\'), Path.GetFullPath(b).TrimEnd('\\'), StringComparison.OrdinalIgnoreCase);
        }

        internal static bool Overlap(string a, string b)
        {
            return SameDirectory(a, b) || a.StartsWith(b.TrimEnd('\\') + "\\", StringComparison.OrdinalIgnoreCase) || b.StartsWith(a.TrimEnd('\\') + "\\", StringComparison.OrdinalIgnoreCase);
        }

        internal static void RequireSameUpgradeDirectory(string chosen, string installed)
        {
            if (!String.IsNullOrEmpty(installed) && !SameDirectory(chosen, installed))
                throw new InvalidOperationException("已检测到现有安装。请在原安装目录更新，不能在本次安装中迁移目录。");
        }

        internal static string EngineArguments(string directory, bool createShortcuts)
        {
            // NSIS /D is deliberately last and unquoted, including paths with spaces.
            string normalized = NormalizeDirectory(directory, null);
            return "/S" + (createShortcuts ? "" : " /NS") + " /D=" + normalized;
        }

        internal static IEnumerable<string> DataDirectories(string profile, string local, string roaming, string codexOverride, string dataOverride)
        {
            List<string> roots = new List<string>();
            if (!String.IsNullOrEmpty(profile)) roots.Add(Path.Combine(profile, ".codex"));
            if (!String.IsNullOrEmpty(local)) roots.Add(Path.Combine(local, "local.tokenmeow.desktop"));
            if (!String.IsNullOrEmpty(roaming)) roots.Add(Path.Combine(roaming, "local.tokenmeow.desktop"));
            if (!String.IsNullOrEmpty(codexOverride) && Path.IsPathRooted(codexOverride)) roots.Add(codexOverride);
            if (!String.IsNullOrEmpty(dataOverride) && Path.IsPathRooted(dataOverride)) roots.Add(dataOverride);
            return roots;
        }

        internal static string EngineError(int exitCode)
        {
            switch (exitCode)
            {
                case 1: return "安装已中止。应用尚未经过本安装器的完成核验。";
                case 64: return "安装引擎拒绝了启动方式；需要使用受支持的静默入口。";
                case 65: return "Token 喵仍在运行。请正常退出应用，再重新安装；安装器不会强制关闭它。";
                case 66: return "暂时无法确认应用是否仍在运行，已停止安装。请正常退出应用后重试。";
                case 67: return "现有安装的类型、身份或目录不支持本次更新，请保留原安装并检查安装路径。";
                case 68: return "已安装的版本更新，不能降级安装。";
                case 69: return "WebView2 下载或安装失败。请检查网络后重试。";
                default: return "安装引擎未正常完成（退出码 " + exitCode + "）。请检查安装目录并重试。";
            }
        }
    }
}
