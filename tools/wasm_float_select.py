"""Rewrite every floating-point `select` in a WebAssembly module as an integer `select`.

    python tools/wasm_float_select.py in.wasm out.wasm

Works around a wazero compiler bug on amd64 (seen in v1.8.0 through v1.12.0, the latest
release). wazero lowers an f32/f64/v128 `select` to a pseudo-instruction, xmmCMov, that it
tells its register allocator *defines* the destination register -- but a conditional move
leaves the destination unchanged when the condition is false, so it also reads it. Under
register pressure (a value live across a call is enough) the allocator gives the destination
a register that never received the false operand, and `select` returns the true operand, or
whatever else sat there, when the condition is false. The integer `select` (cmove) is
modelled correctly. A minimal case, which wazero's interpreter and every other engine run
correctly:

    t = x - y;  c = f(t) < b;  return select(t, x, c)     -- returns t when c is false

In Inkvec that turned a pattern search's `if x < best { best = x; cur = trial }` into
"always accept", so traces under wazero's compiler differed from the WebAssembly semantics.

The rewrite keeps the semantics bit for bit (reinterpret moves bits, NaN payloads included):

    select<f64>(a, b, c)  ->  f64.reinterpret_i64(select<i64>(i64.reinterpret_f64(a),
                                                              i64.reinterpret_f64(b), c))

done in place with two scratch locals per function (the condition and the second operand).
Finding which `select`s are floating-point needs the operand types, so every function body
is decoded and its operand stack typed. The decoder covers what rustc emits for wasm32
without SIMD (MVP, sign extension, saturating truncation, bulk memory, reference types,
multi-value); anything else -- SIMD in particular -- stops it with an error rather than
passing through unexamined. Standard library only.
"""

from __future__ import annotations

import sys

I32, I64, F32, F64, V128, FUNCREF, EXTERNREF = 0x7F, 0x7E, 0x7D, 0x7C, 0x7B, 0x70, 0x6F
UNKNOWN = None  # a value of the polymorphic stack after an unconditional branch


class Reader:
    def __init__(self, data: bytes, pos: int = 0):
        self.d = data
        self.p = pos

    def byte(self) -> int:
        b = self.d[self.p]
        self.p += 1
        return b

    def u32(self) -> int:
        result = shift = 0
        while True:
            b = self.d[self.p]
            self.p += 1
            result |= (b & 0x7F) << shift
            shift += 7
            if b < 0x80:
                return result

    def s(self) -> int:
        result = shift = 0
        while True:
            b = self.d[self.p]
            self.p += 1
            result |= (b & 0x7F) << shift
            shift += 7
            if b < 0x80:
                if b & 0x40:
                    result -= 1 << shift
                return result

    def name(self) -> str:
        n = self.u32()
        s = self.d[self.p : self.p + n].decode()
        self.p += n
        return s


def uleb(v: int) -> bytes:
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


# Operand-stack effect of the plain numeric opcodes: opcode -> (pops, pushes).
NUMERIC: dict[int, tuple[tuple[int, ...], tuple[int, ...]]] = {0x45: ((I32,), (I32,)), 0x50: ((I64,), (I32,))}
for op in range(0x46, 0x50):
    NUMERIC[op] = ((I32, I32), (I32,))
for op in range(0x51, 0x5B):
    NUMERIC[op] = ((I64, I64), (I32,))
for op in range(0x5B, 0x61):
    NUMERIC[op] = ((F32, F32), (I32,))
for op in range(0x61, 0x67):
    NUMERIC[op] = ((F64, F64), (I32,))
for op in range(0x67, 0x6A):
    NUMERIC[op] = ((I32,), (I32,))
for op in range(0x6A, 0x79):
    NUMERIC[op] = ((I32, I32), (I32,))
for op in range(0x79, 0x7C):
    NUMERIC[op] = ((I64,), (I64,))
for op in range(0x7C, 0x8B):
    NUMERIC[op] = ((I64, I64), (I64,))
for op in range(0x8B, 0x92):
    NUMERIC[op] = ((F32,), (F32,))
for op in range(0x92, 0x99):
    NUMERIC[op] = ((F32, F32), (F32,))
for op in range(0x99, 0xA0):
    NUMERIC[op] = ((F64,), (F64,))
for op in range(0xA0, 0xA7):
    NUMERIC[op] = ((F64, F64), (F64,))
for op, (src, dst) in {
    0xA7: (I64, I32), 0xA8: (F32, I32), 0xA9: (F32, I32), 0xAA: (F64, I32), 0xAB: (F64, I32),
    0xAC: (I32, I64), 0xAD: (I32, I64), 0xAE: (F32, I64), 0xAF: (F32, I64), 0xB0: (F64, I64),
    0xB1: (F64, I64), 0xB2: (I32, F32), 0xB3: (I32, F32), 0xB4: (I64, F32), 0xB5: (I64, F32),
    0xB6: (F64, F32), 0xB7: (I32, F64), 0xB8: (I32, F64), 0xB9: (I64, F64), 0xBA: (I64, F64),
    0xBB: (F32, F64), 0xBC: (F32, I32), 0xBD: (F64, I64), 0xBE: (I32, F32), 0xBF: (I64, F64),
    0xC0: (I32, I32), 0xC1: (I32, I32), 0xC2: (I64, I64), 0xC3: (I64, I64), 0xC4: (I64, I64),
}.items():
    NUMERIC[op] = ((src,), (dst,))

LOADS = {0x28: I32, 0x29: I64, 0x2A: F32, 0x2B: F64, 0x2C: I32, 0x2D: I32, 0x2E: I32, 0x2F: I32,
         0x30: I64, 0x31: I64, 0x32: I64, 0x33: I64, 0x34: I64, 0x35: I64}
STORES = {0x36: I32, 0x37: I64, 0x38: F32, 0x39: F64, 0x3A: I32, 0x3B: I32, 0x3C: I64, 0x3D: I64, 0x3E: I64}
SAT_TRUNC = {0: (F32, I32), 1: (F32, I32), 2: (F64, I32), 3: (F64, I32),
             4: (F32, I64), 5: (F32, I64), 6: (F64, I64), 7: (F64, I64)}

# (float type) -> (to-integer reinterpret, back-to-float reinterpret)
REINTERPRET = {F64: (0xBD, 0xBF), F32: (0xBC, 0xBE)}


class Module:
    def __init__(self, data: bytes):
        if data[:8] != b"\0asm\x01\0\0\0":
            raise SystemExit("not a WebAssembly 1.0 binary module")
        self.data = data
        self.sections: list[tuple[int, int, int]] = []  # (id, start, end) of the contents
        r = Reader(data, 8)
        while r.p < len(data):
            sid = r.byte()
            size = r.u32()
            self.sections.append((sid, r.p, r.p + size))
            r.p += size
        self.types: list[tuple[tuple[int, ...], tuple[int, ...]]] = []
        self.funcs: list[int] = []  # type index of every function, imports first
        self.globals: list[int] = []
        self.tables: list[int] = []
        for sid, start, end in self.sections:
            r = Reader(data, start)
            if sid == 1:
                for _ in range(r.u32()):
                    if r.byte() != 0x60:
                        raise SystemExit("unsupported type form")
                    params = tuple(r.byte() for _ in range(r.u32()))
                    results = tuple(r.byte() for _ in range(r.u32()))
                    self.types.append((params, results))
            elif sid == 2:
                for _ in range(r.u32()):
                    r.name()
                    r.name()
                    kind = r.byte()
                    if kind == 0:
                        self.funcs.append(r.u32())
                    elif kind == 1:
                        self.tables.append(r.byte())
                        self._limits(r)
                    elif kind == 2:
                        self._limits(r)
                    elif kind == 3:
                        self.globals.append(r.byte())
                        r.byte()
                    else:
                        raise SystemExit(f"unsupported import kind {kind}")
            elif sid == 3:
                self.funcs.extend(r.u32() for _ in range(r.u32()))
            elif sid == 4:
                for _ in range(r.u32()):
                    self.tables.append(r.byte())
                    self._limits(r)
            elif sid == 6:
                for _ in range(r.u32()):
                    self.globals.append(r.byte())
                    r.byte()
                    self._skip_const_expr(r)

    @staticmethod
    def _limits(r: Reader) -> None:
        flags = r.byte()
        r.u32()
        if flags & 1:
            r.u32()

    @staticmethod
    def _skip_const_expr(r: Reader) -> None:
        while True:
            op = r.byte()
            if op == 0x0B:
                return
            if op in (0x41, 0x42):
                r.s()
            elif op == 0x43:
                r.p += 4
            elif op == 0x44:
                r.p += 8
            elif op in (0x23, 0xD2):
                r.u32()
            elif op == 0xD0:
                r.byte()
            elif op in (0x6A, 0x6B, 0x6C, 0x7C, 0x7D, 0x7E):  # extended-const arithmetic
                pass
            else:
                raise SystemExit(f"unsupported constant expression opcode {op:#x}")

    def block_type(self, r: Reader) -> tuple[tuple[int, ...], tuple[int, ...]]:
        b = r.d[r.p]
        if b == 0x40:
            r.p += 1
            return (), ()
        if b in (I32, I64, F32, F64, V128, FUNCREF, EXTERNREF):
            r.p += 1
            return (), (b,)
        return self.types[r.s()]


def rewrite_body(m: Module, body: bytes, func_type: tuple) -> tuple[bytes, int]:
    """One function body with its float selects rewritten, and how many there were."""
    params, results = func_type
    r = Reader(body)
    local_groups = []
    locals_ = list(params)
    for _ in range(r.u32()):
        n = r.u32()
        t = r.byte()
        local_groups.append((n, t))
        locals_.extend([t] * n)
    code_start = r.p
    base = len(locals_)
    cond_local, f64_local, f32_local = base, base + 1, base + 2

    out = bytearray()
    copied = code_start  # body[copied:r.p] is still to be copied verbatim
    rewrites = 0
    stack: list = []
    # control frames: [height, params, results, unreachable]
    frames = [[0, (), results, False]]

    def pop(expect=None):
        frame = frames[-1]
        if len(stack) == frame[0]:
            if frame[3]:
                return UNKNOWN
            raise SystemExit(f"operand stack underflow at byte {r.p}")
        t = stack.pop()
        if expect is not None and t is not UNKNOWN and t != expect:
            raise SystemExit(f"type mismatch at byte {r.p}: {t:#x} where {expect:#x} expected")
        return t

    def unreachable():
        frame = frames[-1]
        del stack[frame[0]:]
        frame[3] = True

    d = body
    n = len(d)
    while r.p < n:
        at = r.p
        op = d[r.p]
        r.p += 1
        if op in NUMERIC:
            pops, pushes = NUMERIC[op]
            for t in reversed(pops):
                pop(t)
            stack.extend(pushes)
        elif op == 0x20:
            stack.append(locals_[r.u32()])
        elif op == 0x21:
            pop(locals_[r.u32()])
        elif op == 0x22:
            t = locals_[r.u32()]
            pop(t)
            stack.append(t)
        elif op in LOADS:
            r.u32()
            r.u32()
            pop(I32)
            stack.append(LOADS[op])
        elif op in STORES:
            r.u32()
            r.u32()
            pop(STORES[op])
            pop(I32)
        elif op == 0x41:
            r.s()
            stack.append(I32)
        elif op == 0x42:
            r.s()
            stack.append(I64)
        elif op == 0x43:
            r.p += 4
            stack.append(F32)
        elif op == 0x44:
            r.p += 8
            stack.append(F64)
        elif op in (0x1B, 0x1C):
            typed = None
            if op == 0x1C:
                count = r.u32()
                if count != 1:
                    raise SystemExit("select with several result types")
                typed = r.byte()
            pop(I32)
            t2 = pop(typed)
            t1 = pop(typed)
            t = typed if typed is not None else (t1 if t1 is not UNKNOWN else t2)
            stack.append(t)
            if t == V128:
                raise SystemExit("v128 select: build without SIMD (see the module docstring)")
            if t in REINTERPRET and not frames[-1][3]:
                to_int, to_float = REINTERPRET[t]
                scratch = f64_local if t == F64 else f32_local
                out += d[copied:at]
                out += b"\x21" + uleb(cond_local)
                out += b"\x21" + uleb(scratch)
                out.append(to_int)
                out += b"\x20" + uleb(scratch)
                out.append(to_int)
                out += b"\x20" + uleb(cond_local)
                out.append(0x1B)
                out.append(to_float)
                copied = r.p
                rewrites += 1
        elif op == 0x10:
            p, res = m.types[m.funcs[r.u32()]]
            for t in reversed(p):
                pop(t)
            stack.extend(res)
        elif op == 0x11:
            p, res = m.types[r.u32()]
            r.u32()
            pop(I32)
            for t in reversed(p):
                pop(t)
            stack.extend(res)
        elif op in (0x02, 0x03, 0x04):
            bparams, bresults = m.block_type(r)
            if op == 0x04:
                pop(I32)
            for t in reversed(bparams):
                pop(t)
            frames.append([len(stack), bparams, bresults, False, op])
            stack.extend(bparams)
        elif op == 0x05:
            frame = frames[-1]
            del stack[frame[0]:]
            stack.extend(frame[1])
            frame[3] = False
        elif op == 0x0B:
            frame = frames.pop()
            del stack[frame[0]:]
            stack.extend(frame[2])
            if not frames:
                if r.p != n:
                    raise SystemExit("code after the function's end")
                break
        elif op == 0x0C:
            r.u32()
            unreachable()
        elif op == 0x0D:
            r.u32()
            pop(I32)
        elif op == 0x0E:
            for _ in range(r.u32()):
                r.u32()
            r.u32()
            pop(I32)
            unreachable()
        elif op in (0x00, 0x0F):
            unreachable()
        elif op == 0x01:
            pass
        elif op == 0x1A:
            pop()
        elif op == 0x23:
            stack.append(m.globals[r.u32()])
        elif op == 0x24:
            pop(m.globals[r.u32()])
        elif op == 0x25:
            t = m.tables[r.u32()]
            pop(I32)
            stack.append(t)
        elif op == 0x26:
            r.u32()
            pop()
            pop(I32)
        elif op == 0x3F:
            r.byte()
            stack.append(I32)
        elif op == 0x40:
            r.byte()
            pop(I32)
            stack.append(I32)
        elif op == 0xD0:
            stack.append(r.byte())
        elif op == 0xD1:
            pop()
            stack.append(I32)
        elif op == 0xD2:
            r.u32()
            stack.append(FUNCREF)
        elif op == 0xFC:
            sub = r.u32()
            if sub in SAT_TRUNC:
                src, dst = SAT_TRUNC[sub]
                pop(src)
                stack.append(dst)
            elif sub == 8:  # memory.init
                r.u32()
                r.byte()
                pop(I32), pop(I32), pop(I32)
            elif sub == 9:  # data.drop
                r.u32()
            elif sub == 10:  # memory.copy
                r.byte()
                r.byte()
                pop(I32), pop(I32), pop(I32)
            elif sub == 11:  # memory.fill
                r.byte()
                pop(I32), pop(I32), pop(I32)
            elif sub == 12:  # table.init
                r.u32()
                r.u32()
                pop(I32), pop(I32), pop(I32)
            elif sub == 13:  # elem.drop
                r.u32()
            elif sub == 14:  # table.copy
                r.u32()
                r.u32()
                pop(I32), pop(I32), pop(I32)
            elif sub == 15:  # table.grow
                r.u32()
                pop(I32)
                pop()
                stack.append(I32)
            elif sub == 16:  # table.size
                r.u32()
                stack.append(I32)
            elif sub == 17:  # table.fill
                r.u32()
                pop(I32)
                pop()
                pop(I32)
            else:
                raise SystemExit(f"unsupported 0xfc opcode {sub}")
        else:
            raise SystemExit(f"unsupported opcode {op:#x} (SIMD, threads, exceptions or tail "
                             f"calls are not handled; see the module docstring)")
    if frames:
        raise SystemExit("function body ends inside a block")
    if not rewrites:
        return body, 0
    out += d[copied:]
    header = bytearray(uleb(len(local_groups) + 3))
    for count, t in local_groups:
        header += uleb(count) + bytes([t])
    header += b"\x01\x7f\x01\x7c\x01\x7d"  # the condition, an f64 and an f32 scratch local
    return bytes(header) + bytes(out), rewrites


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__.splitlines()[2].strip(), file=sys.stderr)
        return 2
    data = open(sys.argv[1], "rb").read()
    m = Module(data)
    out = bytearray(data[:8])
    total = 0
    n_imported = len(m.funcs) - sum(
        Reader(data, s).u32() for sid, s, _ in m.sections if sid == 10
    )
    for sid, start, end in m.sections:
        if sid != 10:
            out.append(sid)
            out += uleb(end - start)
            out += data[start:end]
            continue
        r = Reader(data, start)
        count = r.u32()
        bodies = bytearray(uleb(count))
        for i in range(count):
            size = r.u32()
            body = data[r.p : r.p + size]
            r.p += size
            new, k = rewrite_body(m, body, m.types[m.funcs[n_imported + i]])
            total += k
            bodies += uleb(len(new)) + new
        out.append(10)
        out += uleb(len(bodies))
        out += bodies
    open(sys.argv[2], "wb").write(out)
    print(f"rewrote {total} floating-point select(s) as integer selects")
    return 0


if __name__ == "__main__":
    sys.exit(main())
