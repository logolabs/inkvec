package inkvec

import (
	"context"
	"math"
	"testing"

	"github.com/tetratelabs/wazero"
)

// selectRepro is the smallest module found that wazero's amd64 compiler gets wrong (v1.8.0
// through v1.12.0): f(a, b, x, y) = select(t, x, g(t) < b) with t = x - y, where
// g(v) = v > 1 ? v*0.5 : v+3. With the condition false it returns t instead of x. The
// module this package embeds has its floating-point selects rewritten as integer ones by
// tools/wasm_wazero_fixes.py, which is why it is not affected.
func selectRepro() []byte {
	f64c := func(f float64) []byte {
		b := []byte{0x44}
		for i := range 8 {
			b = append(b, byte(math.Float64bits(f)>>(8*i)))
		}
		return b
	}
	cat := func(parts ...[]byte) []byte {
		var o []byte
		for _, p := range parts {
			o = append(o, p...)
		}
		return o
	}
	section := func(id byte, body []byte) []byte { return cat([]byte{id, byte(len(body))}, body) }
	fn := func(code []byte) []byte { return cat([]byte{byte(len(code))}, code) }
	f := cat([]byte{2, 1, 0x7f, 1, 0x7c}, // locals: $4 i32, $5 f64
		[]byte{0x20, 2, 0x20, 3, 0xa1, 0x22, 5},       // t = x - y
		[]byte{0x10, 1, 0x20, 1, 0x63, 0x21, 4},       // c = g(t) < b
		[]byte{0x20, 5, 0x20, 2, 0x20, 4, 0x1b, 0x0b}) // select(t, x, c)
	g := cat([]byte{0, 0x20, 0}, f64c(1), []byte{0x64, 0x04, 0x7c, 0x20, 0}, f64c(0.5),
		[]byte{0xa2, 0x05, 0x20, 0}, f64c(3), []byte{0xa0, 0x0b, 0x0b})
	return cat([]byte{0, 'a', 's', 'm', 1, 0, 0, 0},
		section(1, []byte{2, 0x60, 4, 0x7c, 0x7c, 0x7c, 0x7c, 1, 0x7c, 0x60, 1, 0x7c, 1, 0x7c}),
		section(3, []byte{2, 0, 1}),
		section(7, []byte{1, 1, 'f', 0, 0}),
		section(10, cat([]byte{2}, fn(f), fn(g))))
}

// TestWazeroFloatSelect reports whether wazero still has the bug the build works around. It
// does not fail when the bug is present (that is expected); it says when a wazero upgrade has
// fixed it, so the rewrite in tools/build_go_wasm.sh could go.
func TestWazeroFloatSelect(t *testing.T) {
	ctx := context.Background()
	results := map[string]float64{}
	for name, rc := range map[string]wazero.RuntimeConfig{
		"compiler":    wazero.NewRuntimeConfig(),
		"interpreter": wazero.NewRuntimeConfigInterpreter(),
	} {
		r := wazero.NewRuntimeWithConfig(ctx, rc)
		defer r.Close(ctx)
		m, err := r.Instantiate(ctx, selectRepro())
		if err != nil {
			t.Fatal(err)
		}
		// a=0.3 b=0.3 x=0.8 y=2.5: t=-1.7, g(t)=1.3, 1.3 < 0.3 is false, so the answer is x.
		res, err := m.ExportedFunction("f").Call(ctx,
			math.Float64bits(0.3), math.Float64bits(0.3), math.Float64bits(0.8), math.Float64bits(2.5))
		if err != nil {
			t.Fatal(err)
		}
		results[name] = math.Float64frombits(res[0])
	}
	if results["interpreter"] != 0.8 {
		t.Fatalf("the interpreter returned %v, want 0.8", results["interpreter"])
	}
	if results["compiler"] != 0.8 {
		t.Logf("wazero's compiler still mis-lowers a float select (returned %v, want 0.8); "+
			"tools/wasm_wazero_fixes.py is still needed", results["compiler"])
	} else {
		t.Logf("wazero's compiler gets the float select right on this platform")
	}
}

// remainderRepro is a small module wazero's arm64 compiler gets wrong (v1.12.0): f(a, b, n)
// computes forty values from a, then b rem_u n, then returns the remainder (plus the forty
// values times zero, which keeps them live across it). wazero defines a remainder's register
// twice (a division, then a multiply-subtract) around a branch to its division-by-zero exit;
// the register pressure makes it spill the value in between, the spill is stored after the
// second definition, and the multiply-subtract reloads a quotient that was never stored:
// 1000 rem_u 256 comes out as 1000. The module this package embeds has its remainders
// rewritten as divisions by tools/wasm_wazero_fixes.py, which is why it is not affected.
func remainderRepro() []byte {
	const live = 40
	uleb := func(v int) []byte {
		var o []byte
		for {
			b := byte(v & 0x7f)
			v >>= 7
			if v == 0 {
				return append(o, b)
			}
			o = append(o, b|0x80)
		}
	}
	i32const := func(v int) []byte { // non-negative values below 8192
		if v < 64 {
			return []byte{0x41, byte(v)}
		}
		return []byte{0x41, byte(v&0x7f) | 0x80, byte(v >> 7)}
	}
	cat := func(parts ...[]byte) []byte {
		var o []byte
		for _, p := range parts {
			o = append(o, p...)
		}
		return o
	}
	rem := 3 + live // locals: a, b, n, the live values, the remainder
	code := cat([]byte{1}, uleb(live+1), []byte{0x7f})
	for k := range live { // v_k = a*(2k+3) + 7k+1
		code = cat(code, []byte{0x20, 0}, i32const(2*k+3), []byte{0x6c}, i32const(7*k+1),
			[]byte{0x6a, 0x21}, uleb(3+k))
	}
	code = cat(code, []byte{0x20, 1, 0x20, 2, 0x70, 0x21}, uleb(rem)) // rem = b rem_u n
	code = cat(code, i32const(0))
	for k := range live {
		code = cat(code, []byte{0x20}, uleb(3+k), []byte{0x73}) // xor v_k
	}
	code = cat(code, i32const(0), []byte{0x6c, 0x20}, uleb(rem), []byte{0x6a, 0x0b})
	section := func(id byte, body []byte) []byte { return cat([]byte{id}, uleb(len(body)), body) }
	return cat([]byte{0, 'a', 's', 'm', 1, 0, 0, 0},
		section(1, []byte{1, 0x60, 3, 0x7f, 0x7f, 0x7f, 1, 0x7f}),
		section(3, []byte{1, 0}),
		section(7, []byte{1, 1, 'f', 0, 0}),
		section(10, cat([]byte{1}, uleb(len(code)), code)))
}

// TestWazeroRemainder reports whether wazero still has the remainder bug the build works
// around; like TestWazeroFloatSelect, it fails only if the interpreter is wrong.
func TestWazeroRemainder(t *testing.T) {
	ctx := context.Background()
	results := map[string]uint32{}
	for name, rc := range map[string]wazero.RuntimeConfig{
		"compiler":    wazero.NewRuntimeConfig(),
		"interpreter": wazero.NewRuntimeConfigInterpreter(),
	} {
		r := wazero.NewRuntimeWithConfig(ctx, rc)
		defer r.Close(ctx)
		m, err := r.Instantiate(ctx, remainderRepro())
		if err != nil {
			t.Fatal(err)
		}
		res, err := m.ExportedFunction("f").Call(ctx, 5, 1000, 256)
		if err != nil {
			t.Fatal(err)
		}
		results[name] = uint32(res[0])
	}
	if results["interpreter"] != 232 {
		t.Fatalf("the interpreter returned %d, want 1000 rem 256 = 232", results["interpreter"])
	}
	if results["compiler"] != 232 {
		t.Logf("wazero's compiler still mis-lowers an integer remainder (returned %d, want 232); "+
			"tools/wasm_wazero_fixes.py is still needed", results["compiler"])
	} else {
		t.Logf("wazero's compiler gets the remainder right on this platform")
	}
}
