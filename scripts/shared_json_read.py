"""Bounded Windows JSON reads compatible with atomic rename writers.

No cache and no fallback document. Only explicit Win32 sharing/lock/delete-
pending errors may retry; ACL/access-denied and malformed content fail closed.
"""
import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import json
import os
from pathlib import Path
import stat
import time

FILETIME_EPOCH = 116444736000000000
RETRYABLE_WINERRORS = frozenset((32, 33, 303))
MAX_RETRY_SECONDS = 0.100


class _FileInfo(ctypes.Structure):
    _fields_ = [("attributes", wintypes.DWORD), ("created", wintypes.FILETIME),
                ("accessed", wintypes.FILETIME), ("written", wintypes.FILETIME),
                ("volume", wintypes.DWORD), ("size_high", wintypes.DWORD),
                ("size_low", wintypes.DWORD), ("links", wintypes.DWORD),
                ("index_high", wintypes.DWORD), ("index_low", wintypes.DWORD)]


if os.name == "nt":
    _kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    _kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                  ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    _kernel.CreateFileW.restype = wintypes.HANDLE
    _kernel.GetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.POINTER(_FileInfo)]
    _kernel.GetFileInformationByHandle.restype = wintypes.BOOL
    _kernel.GetFinalPathNameByHandleW.argtypes = [wintypes.HANDLE, wintypes.LPWSTR, wintypes.DWORD, wintypes.DWORD]
    _kernel.GetFinalPathNameByHandleW.restype = wintypes.DWORD
    _kernel.GetFileType.argtypes = [wintypes.HANDLE]
    _kernel.GetFileType.restype = wintypes.DWORD
    _kernel.ReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                                ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    _kernel.ReadFile.restype = wintypes.BOOL
    _kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    _kernel.CloseHandle.restype = wintypes.BOOL


@dataclass(frozen=True)
class JsonDocument:
    value: object
    modified_unix_ns: int
    byte_count: int
    file_identity: tuple
    attempts: int
    completed_unix_ns: int


def _absolute_without_reparse(value):
    path = Path(os.path.abspath(value))
    for part in reversed((path, *path.parents)):
        info = os.lstat(part)
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError("reparse_or_symlink_rejected")
    return path


def _canonical_name(value):
    text = str(value)
    if text.startswith("\\\\?\\UNC\\"):
        text = "\\\\" + text[8:]
    elif text.startswith("\\\\?\\"):
        text = text[4:]
    return os.path.normcase(os.path.abspath(text))


def _info(handle):
    info = _FileInfo()
    if not _kernel.GetFileInformationByHandle(handle, ctypes.byref(info)):
        raise ctypes.WinError(ctypes.get_last_error())
    if info.attributes & (0x400 | 0x10):
        raise ValueError("reparse_or_non_regular_file_rejected")
    return ((info.size_high << 32) | info.size_low,
            (info.written.dwHighDateTime << 32) | info.written.dwLowDateTime,
            (info.volume, info.index_high, info.index_low))


def _read_once(path, maximum):
    path = _absolute_without_reparse(path)
    # GENERIC_READ; FILE_SHARE_READ | WRITE | DELETE; OPEN_EXISTING;
    # OPEN_REPARSE_POINT prevents silently following a link at the final entry.
    handle = _kernel.CreateFileW(str(path), 0x80000000, 0x7, None, 3, 0x00200000, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if _kernel.GetFileType(handle) != 1:
            raise ValueError("non_disk_file_rejected")
        buffer = ctypes.create_unicode_buffer(32768)
        length = _kernel.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
        if not length:
            raise ctypes.WinError(ctypes.get_last_error())
        if length >= len(buffer) or _canonical_name(buffer.value) != _canonical_name(path):
            raise ValueError("opened_file_path_differs_or_exceeds_bound")
        before = _info(handle)
        if before[0] > maximum:
            raise ValueError("control_document_exceeds_bound")
        payload = bytearray()
        while len(payload) <= maximum:
            block = ctypes.create_string_buffer(min(65536, maximum + 1 - len(payload)))
            count = wintypes.DWORD()
            if not _kernel.ReadFile(handle, block, len(block), ctypes.byref(count), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if count.value == 0:
                break
            payload.extend(block.raw[:count.value])
        after = _info(handle)
        if len(payload) > maximum or before != after or len(payload) != before[0]:
            raise ValueError("control_document_changed_during_read_or_exceeds_bound")
        value = json.loads(payload.decode("utf-8-sig"))
        return value, (before[1] - FILETIME_EPOCH) * 100, len(payload), before[2]
    finally:
        _kernel.CloseHandle(handle)


def read_json_document(path, maximum=2 * 1024 * 1024):
    if os.name != "nt":
        raise OSError("shared JSON observer reads require Windows")
    if type(maximum) is not int or not 0 < maximum <= 8 * 1024 * 1024:
        raise ValueError("invalid_control_document_bound")
    deadline, attempts = time.monotonic() + MAX_RETRY_SECONDS, 0
    while True:
        attempts += 1
        try:
            value, modified_ns, size, identity = _read_once(path, maximum)
            return JsonDocument(value, modified_ns, size, identity, attempts, time.time_ns())
        except OSError as error:
            remaining = deadline - time.monotonic()
            if getattr(error, "winerror", None) not in RETRYABLE_WINERRORS or remaining <= 0:
                raise
            time.sleep(min(0.005, remaining))


def read_json(path, maximum=2 * 1024 * 1024):
    return read_json_document(path, maximum).value
