#!/usr/bin/env python3
"""Report, and optionally enforce, the oldest system a built binary will start on.

A release archive that will not start is the worst kind of bug: the failure is the
dynamic loader's, before `main`, and the only report it produces is a user saying the
download is broken. This reads the binary itself and says what it actually requires.

    abi_floor.py BIN --glibc 2.17 [--glibcxx 3.4.30] [--no-weak-report]
    abi_floor.py BIN --macos 10.12

* ELF: every versioned symbol the binary imports (`.gnu.version_r`), split into the ones
  that must resolve and the weak ones that need not. Rust's standard library references
  `pidfd_spawnp` weakly, for instance: the loader is content to leave it null on an older
  glibc and the program takes its other path, so a weak reference does not raise the
  floor and is reported separately rather than counted.
* Mach-O: `LC_BUILD_VERSION` / `LC_VERSION_MIN_MACOSX`, for each architecture in the file.

Exits non-zero when a floor is exceeded, with a GitHub Actions error annotation. Pure
stdlib, no objdump or otool, so it runs identically on all three runners and locally.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

# ----------------------------------------------------------------------------- ELF ---

SHT_DYNSYM = 11
SHT_GNU_VERSYM = 0x6FFFFFFF
SHT_GNU_VERNEED = 0x6FFFFFFE
SHN_UNDEF = 0
STB_WEAK = 2
VER_NDX_LOCAL = 0
VER_NDX_GLOBAL = 1
VERSYM_HIDDEN = 0x8000


def _version_key(name: str) -> tuple[int, ...]:
    """`GLIBC_2.17` -> (2, 17), so versions sort numerically rather than as text."""
    _, _, digits = name.rpartition("_")
    parts = []
    for piece in digits.split("."):
        parts.append(int(piece) if piece.isdigit() else 0)
    return tuple(parts)


def _cstr(blob: bytes, offset: int) -> str:
    end = blob.index(b"\0", offset)
    return blob[offset:end].decode("utf-8", "replace")


class Elf:
    """Just enough ELF to read the dynamic symbol table and its version requirements."""

    def __init__(self, data: bytes) -> None:
        self.data = data
        if data[4] != 2:
            raise ValueError("only 64-bit ELF is supported")
        self.little = data[5] == 1
        self.end = "<" if self.little else ">"
        # e_type, e_machine, e_version, e_entry, e_phoff, e_shoff, e_flags, e_ehsize,
        # e_phentsize, e_phnum, e_shentsize, e_shnum, e_shstrndx.
        (
            _type,
            _machine,
            _version,
            _entry,
            _phoff,
            self.shoff,
            _flags,
            _ehsize,
            _phentsize,
            _phnum,
            self.shentsize,
            self.shnum,
            self.shstrndx,
        ) = struct.unpack_from(self.end + "HHIQQQIHHHHHH", data, 16)
        self.sections = [self._section(i) for i in range(self.shnum)]
        names = self.sections[self.shstrndx]
        strtab = data[names["offset"] : names["offset"] + names["size"]]
        for section in self.sections:
            section["name"] = _cstr(strtab, section["name_off"])

    def _section(self, index: int) -> dict:
        base = self.shoff + index * self.shentsize
        (
            name_off,
            kind,
            _flags,
            _addr,
            offset,
            size,
            link,
            _info,
            _align,
            entsize,
        ) = struct.unpack_from(self.end + "IIQQQQIIQQ", self.data, base)
        return {
            "name_off": name_off,
            "type": kind,
            "offset": offset,
            "size": size,
            "link": link,
            "entsize": entsize,
        }

    def _by_type(self, kind: int) -> dict | None:
        for section in self.sections:
            if section["type"] == kind:
                return section
        return None

    def _bytes(self, section: dict) -> bytes:
        return self.data[section["offset"] : section["offset"] + section["size"]]

    def requirements(self) -> list[tuple[str, str, bool]]:
        """(symbol, version, weak) for every versioned symbol this binary imports."""
        verneed = self._by_type(SHT_GNU_VERNEED)
        dynsym = self._by_type(SHT_DYNSYM)
        versym = self._by_type(SHT_GNU_VERSYM)
        if not verneed or not dynsym or not versym:
            return []
        strtab = self._bytes(self.sections[verneed["link"]])

        # .gnu.version_r: one entry per needed library, each with a chain of the exact
        # version names wanted from it, keyed by the index .gnu.version stores per symbol.
        versions: dict[int, str] = {}
        blob = self._bytes(verneed)
        offset = 0
        while True:
            _ver, count, _file, aux, nxt = struct.unpack_from(self.end + "HHIII", blob, offset)
            aux_at = offset + aux
            for _ in range(count):
                _hash, _flags, other, name, aux_next = struct.unpack_from(
                    self.end + "IHHII", blob, aux_at
                )
                versions[other & ~VERSYM_HIDDEN] = _cstr(strtab, name)
                if aux_next == 0:
                    break
                aux_at += aux_next
            if nxt == 0:
                break
            offset += nxt

        sym_strtab = self._bytes(self.sections[dynsym["link"]])
        sym_blob = self._bytes(dynsym)
        ver_blob = self._bytes(versym)
        entsize = dynsym["entsize"] or 24
        out: list[tuple[str, str, bool]] = []
        for index in range(len(sym_blob) // entsize):
            name, info, _other, shndx, _value, _size = struct.unpack_from(
                self.end + "IBBHQQ", sym_blob, index * entsize
            )
            if shndx != SHN_UNDEF:
                continue
            (raw,) = struct.unpack_from(self.end + "H", ver_blob, index * 2)
            needed = versions.get(raw & ~VERSYM_HIDDEN)
            if needed is None or (raw & ~VERSYM_HIDDEN) in (VER_NDX_LOCAL, VER_NDX_GLOBAL):
                continue
            out.append((_cstr(sym_strtab, name), needed, (info >> 4) == STB_WEAK))
        return out


def check_elf(path: Path, args: argparse.Namespace) -> int:
    elf = Elf(path.read_bytes())
    wanted = elf.requirements()
    failures = 0

    for prefix, floor in (("GLIBC_", args.glibc), ("GLIBCXX_", args.glibcxx)):
        if floor is None:
            continue
        ceiling = _version_key(prefix + floor)
        strong = [w for w in wanted if w[1].startswith(prefix) and not w[2]]
        weak = [w for w in wanted if w[1].startswith(prefix) and w[2]]
        highest = max((w[1] for w in strong), key=_version_key, default=None)
        print(f"{path.name}: highest required {prefix.rstrip('_')} is {highest or 'none'}; floor is {prefix}{floor}")
        over = sorted({(w[1], w[0]) for w in strong if _version_key(w[1]) > ceiling})
        # Enough to see what pulled the floor up, without burying the annotation.
        for version, symbol in over[-args.list_limit :]:
            print(f"  requires {version}: {symbol}")
        if len(over) > args.list_limit:
            print(f"  ... and {len(over) - args.list_limit} more above the floor")
        if over:
            print(f"::error::{path.name} requires {over[-1][0]}, above the {prefix}{floor} floor")
            failures += 1
        if args.weak_report:
            for version, symbol in sorted({(w[1], w[0]) for w in weak if _version_key(w[1]) > ceiling}):
                print(f"  (weak, does not raise the floor) {version}: {symbol}")
    return failures


# -------------------------------------------------------------------------- Mach-O ---

FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
MH_MAGIC_64 = 0xFEEDFACF
MH_CIGAM_64 = 0xCFFAEDFE
LC_VERSION_MIN_MACOSX = 0x24
LC_BUILD_VERSION = 0x32
PLATFORM_MACOS = 1


def _macho_version(raw: int) -> str:
    return f"{raw >> 16}.{(raw >> 8) & 0xFF}.{raw & 0xFF}"


def _macho_slices(data: bytes) -> list[bytes]:
    (magic,) = struct.unpack_from(">I", data, 0)
    if magic not in (FAT_MAGIC, FAT_MAGIC_64):
        return [data]
    wide = magic == FAT_MAGIC_64
    (count,) = struct.unpack_from(">I", data, 4)
    out = []
    at = 8
    for _ in range(count):
        if wide:
            _cpu, _sub, offset, size = struct.unpack_from(">iiQQ", data, at)
            at += 32
        else:
            _cpu, _sub, offset, size, _align = struct.unpack_from(">iiIII", data, at)
            at += 20
        out.append(data[offset : offset + size])
    return out


def check_macho(path: Path, args: argparse.Namespace) -> int:
    failures = 0
    want = tuple(int(p) for p in args.macos.split("."))
    for slice_data in _macho_slices(path.read_bytes()):
        (magic,) = struct.unpack_from("<I", slice_data, 0)
        end = "<" if magic == MH_MAGIC_64 else ">"
        _magic, cputype, _sub, _type, ncmds, _size, _flags, _res = struct.unpack_from(
            end + "IiiIIIII", slice_data, 0
        )
        arch = {0x0100000C: "arm64", 0x01000007: "x86_64"}.get(cputype & 0xFFFFFFFF, str(cputype))
        at = 32
        minos = None
        for _ in range(ncmds):
            cmd, size = struct.unpack_from(end + "II", slice_data, at)
            if cmd == LC_BUILD_VERSION:
                platform, raw, _sdk, _ntools = struct.unpack_from(end + "IIII", slice_data, at + 8)
                if platform == PLATFORM_MACOS:
                    minos = _macho_version(raw)
            elif cmd == LC_VERSION_MIN_MACOSX:
                (raw,) = struct.unpack_from(end + "I", slice_data, at + 8)
                minos = _macho_version(raw)
            at += size
        print(f"{path.name} ({arch}): minimum macOS is {minos or 'unset'}; floor is {args.macos}")
        if minos is None:
            print(f"::error::{path.name} ({arch}) declares no minimum macOS version")
            failures += 1
        elif tuple(int(p) for p in minos.split(".")) > want + (0,) * (3 - len(want)):
            print(f"::error::{path.name} ({arch}) requires macOS {minos}, above the {args.macos} floor")
            failures += 1
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--glibc", help="highest glibc symbol version allowed, e.g. 2.17")
    parser.add_argument("--glibcxx", help="highest libstdc++ symbol version allowed, e.g. 3.4.30")
    parser.add_argument("--macos", help="highest minimum macOS version allowed, e.g. 10.12")
    parser.add_argument(
        "--list-limit",
        type=int,
        default=12,
        help="how many offending symbols to name before summarising the rest",
    )
    parser.add_argument(
        "--no-weak-report",
        dest="weak_report",
        action="store_false",
        help="do not list the weak references that exceed the floor without raising it",
    )
    args = parser.parse_args()

    if not args.binary.is_file():
        print(f"::error::{args.binary} is not a file")
        return 1
    head = args.binary.read_bytes()[:4]
    if head == b"\x7fELF":
        if args.glibc is None and args.glibcxx is None:
            print(f"::error::{args.binary} is ELF; pass --glibc (and optionally --glibcxx)")
            return 1
        return 1 if check_elf(args.binary, args) else 0
    # Mach-O, either byte order, thin or fat. A little-endian 64-bit image starts with
    # MH_MAGIC_64 written backwards, which is what MH_CIGAM_64 is.
    if struct.unpack(">I", head)[0] in (FAT_MAGIC, FAT_MAGIC_64, MH_MAGIC_64, MH_CIGAM_64):
        if args.macos is None:
            print(f"::error::{args.binary} is Mach-O; pass --macos")
            return 1
        return 1 if check_macho(args.binary, args) else 0
    print(f"::error::{args.binary} is neither ELF nor Mach-O; nothing to check")
    return 1


if __name__ == "__main__":
    sys.exit(main())
