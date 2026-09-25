package inkvec

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/draw"
	"image/png"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"
)

// testEngine is the package's shared engine: compiling the module takes seconds, so the
// tests share one compilation.
func testEngine(t testing.TB) *Engine {
	t.Helper()
	e, err := defaultEngine()
	if err != nil {
		t.Fatal(err)
	}
	return e
}

// cachedEngine is an engine of its own that reuses a compilation cache on disk.
func cachedEngine(t testing.TB, cfg Config) *Engine {
	t.Helper()
	cfg.CompilationCacheDir = filepath.Join(os.TempDir(), "inkvec-go-test-cache")
	e, err := NewEngine(context.Background(), &cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = e.Close(context.Background()) })
	return e
}

func fixture(t testing.TB, name string) []byte {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(contractDir(t), name))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestModuleDescribesItself(t *testing.T) {
	if got := BuildTarget(); got != "wasm32-wasip1" {
		t.Errorf("BuildTarget() = %q", got)
	}
	if !regexp.MustCompile(`^\d+\.\d+\.\d+`).MatchString(Version()) {
		t.Errorf("Version() = %q", Version())
	}
	var schema struct {
		Title      string                    `json:"title"`
		Properties map[string]map[string]any `json:"properties"`
	}
	if err := json.Unmarshal([]byte(OptionsSchema()), &schema); err != nil {
		t.Fatal(err)
	}
	if schema.Title != "InkvecOptions" || len(schema.Properties) == 0 {
		t.Fatalf("schema: %+v", schema)
	}
	var defaults map[string]any
	if err := json.Unmarshal([]byte(Defaults()), &defaults); err != nil {
		t.Fatal(err)
	}
	for name, prop := range schema.Properties {
		if !reflect.DeepEqual(defaults[name], prop["default"]) {
			t.Errorf("%s: default %v, schema says %v", name, defaults[name], prop["default"])
		}
	}
}

// The version comes from the workspace Cargo.toml, through the module: a stale inkvec.wasm
// shows up here.
func TestVersionIsTheWorkspaceVersion(t *testing.T) {
	toml, err := os.ReadFile("../../Cargo.toml")
	if err != nil {
		t.Skip("not in the monorepo")
	}
	m := regexp.MustCompile(`(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"`).FindSubmatch(toml)
	if m == nil {
		t.Fatal("no [workspace.package] version in Cargo.toml")
	}
	if Version() != string(m[1]) {
		t.Errorf("module version %q, Cargo.toml %q: rebuild with tools/build_go_wasm.sh", Version(), m[1])
	}
}

// The generated struct and the module's schema name the same options: a stale
// options_generated.go (or inkvec.wasm) shows up here.
func TestOptionsStructMatchesTheSchema(t *testing.T) {
	var schema struct {
		Properties map[string]struct {
			Type string `json:"type"`
		} `json:"properties"`
	}
	if err := json.Unmarshal([]byte(OptionsSchema()), &schema); err != nil {
		t.Fatal(err)
	}
	kinds := map[reflect.Kind]string{reflect.Bool: "boolean", reflect.Int: "integer", reflect.Float64: "number", reflect.String: "string"}
	fields := map[string]bool{}
	rt := reflect.TypeFor[Options]()
	for i := range rt.NumField() {
		f := rt.Field(i)
		name := strings.Split(f.Tag.Get("json"), ",")[0]
		fields[name] = true
		prop, ok := schema.Properties[name]
		if !ok {
			t.Errorf("Options.%s (%s) is not in the schema: regenerate", f.Name, name)
			continue
		}
		if f.Type.Kind() != reflect.Pointer || kinds[f.Type.Elem().Kind()] != prop.Type {
			t.Errorf("Options.%s is %s, the schema says %s", f.Name, f.Type, prop.Type)
		}
	}
	for name := range schema.Properties {
		if !fields[name] {
			t.Errorf("option %s has no field in Options: run python bindings/codegen/generate.py", name)
		}
	}

	// Defaults() fills every field.
	var o Options
	dec := json.NewDecoder(strings.NewReader(Defaults()))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&o); err != nil {
		t.Fatal(err)
	}
	v := reflect.ValueOf(o)
	for i := range v.NumField() {
		if v.Field(i).IsNil() {
			t.Errorf("Defaults() leaves Options.%s nil", rt.Field(i).Name)
		}
	}
}

// Encoded bytes, raw RGBA bytes and pixels decoded in Go give one SVG, and so do typed
// options and the same options as JSON.
func TestInputKinds(t *testing.T) {
	ctx := context.Background()
	pngBytes := fixture(t, "tiny.png")
	raw := fixture(t, "tiny.rgba")

	enc, err := Trace(ctx, pngBytes, nil)
	if err != nil {
		t.Fatal(err)
	}
	if enc.Width != 96 || enc.Height != 96 || !strings.HasPrefix(enc.SVG, "<svg") {
		t.Fatalf("got %dx%d %.40q", enc.Width, enc.Height, enc.SVG)
	}
	fromRaw, err := TraceRGBA(ctx, raw, 96, 96, nil)
	if err != nil {
		t.Fatal(err)
	}
	if fromRaw.SVG != enc.SVG {
		t.Error("TraceRGBA on the raw pixels differs from Trace on the PNG")
	}

	img, err := png.Decode(bytes.NewReader(pngBytes))
	if err != nil {
		t.Fatal(err)
	}
	nrgba := image.NewNRGBA(img.Bounds())
	draw.Draw(nrgba, nrgba.Bounds(), img, img.Bounds().Min, draw.Src)
	fromGo, err := TraceRGBA(ctx, nrgba.Pix, nrgba.Rect.Dx(), nrgba.Rect.Dy(), nil)
	if err != nil {
		t.Fatal(err)
	}
	if fromGo.SVG != enc.SVG {
		t.Error("TraceRGBA on image.NRGBA pixels differs from Trace on the PNG")
	}

	typed, err := Trace(ctx, pngBytes, &Options{
		Colors: Ptr(8), Minify: Ptr(true), NoBackground: Ptr(true), Harmonize: Ptr(false),
	})
	if err != nil {
		t.Fatal(err)
	}
	asJSON, err := TraceJSON(ctx, pngBytes, `{"colors": 8, "minify": true, "no_background": true, "harmonize": false}`)
	if err != nil {
		t.Fatal(err)
	}
	if typed.SVG != asJSON.SVG {
		t.Error("typed options and the same options as JSON differ")
	}
	if typed.SVG == enc.SVG {
		t.Error("the options changed nothing")
	}

	// nil, empty Options, "" and "{}" all mean the defaults.
	for _, got := range []func() (*Traced, error){
		func() (*Traced, error) { return Trace(ctx, pngBytes, &Options{}) },
		func() (*Traced, error) { return TraceJSON(ctx, pngBytes, "") },
		func() (*Traced, error) { return TraceJSON(ctx, pngBytes, "{}") },
		func() (*Traced, error) { return TraceRGBAJSON(ctx, raw, 96, 96, " ") },
	} {
		tr, err := got()
		if err != nil {
			t.Fatal(err)
		}
		if tr.SVG != enc.SVG {
			t.Error("default options differ from nil options")
		}
	}
}

func TestErrors(t *testing.T) {
	ctx := context.Background()
	pngBytes := fixture(t, "tiny.png")
	cases := []struct {
		name    string
		call    func() (*Traced, error)
		kind    error
		code    string
		mention string
	}{
		{"unknown option", func() (*Traced, error) { return TraceJSON(ctx, pngBytes, `{"colours": 8}`) },
			ErrInvalidOptions, "invalid_options", "colours"},
		{"out of range", func() (*Traced, error) { return Trace(ctx, pngBytes, &Options{Colors: Ptr(0)}) },
			ErrInvalidOptions, "invalid_options", "colors"},
		{"wrong type", func() (*Traced, error) { return TraceJSON(ctx, pngBytes, `{"cutout": "yes"}`) },
			ErrInvalidOptions, "invalid_options", "boolean"},
		{"not an object", func() (*Traced, error) { return TraceJSON(ctx, pngBytes, `[1]`) },
			ErrInvalidOptions, "invalid_options", ""},
		{"malformed", func() (*Traced, error) { return TraceJSON(ctx, pngBytes, `{not json`) },
			ErrInvalidOptions, "invalid_options", ""},
		{"NUL in JSON", func() (*Traced, error) { return TraceJSON(ctx, pngBytes, "{}\x00") },
			ErrInvalidOptions, "invalid_options", "NUL"},
		{"NaN", func() (*Traced, error) { return Trace(ctx, pngBytes, &Options{Merge: Ptr(math.NaN())}) },
			ErrInvalidOptions, "invalid_options", "NaN"},
		{"not an image", func() (*Traced, error) { return Trace(ctx, []byte("not an image"), nil) },
			ErrInvalidImage, "invalid_image", ""},
		{"empty input", func() (*Traced, error) { return Trace(ctx, nil, nil) },
			ErrInvalidImage, "invalid_image", ""},
		{"rgba size mismatch", func() (*Traced, error) { return TraceRGBA(ctx, make([]byte, 16), 3, 1, nil) },
			ErrInvalidImage, "invalid_image", "12 bytes"},
		{"zero size", func() (*Traced, error) { return TraceRGBA(ctx, nil, 0, 0, nil) },
			ErrInvalidImage, "invalid_image", "nonzero"},
		{"negative size", func() (*Traced, error) { return TraceRGBA(ctx, nil, -1, 4, nil) },
			ErrInvalidImage, "invalid_image", "-1x4"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			tr, err := c.call()
			if tr != nil || !errors.Is(err, c.kind) {
				t.Fatalf("got %v, %v; want %v", tr, err, c.kind)
			}
			for _, other := range []error{ErrInvalidImage, ErrInvalidOptions, ErrInternal} {
				if other != c.kind && errors.Is(err, other) {
					t.Errorf("%v also matches %v", err, other)
				}
			}
			var ie *Error
			if !errors.As(err, &ie) || ie.Code != c.code {
				t.Fatalf("not an *Error with code %s: %#v", c.code, err)
			}
			if !strings.Contains(err.Error(), c.mention) || !strings.HasPrefix(err.Error(), "inkvec: ") {
				t.Errorf("message %q should mention %q", err.Error(), c.mention)
			}
		})
	}
	// A failed call leaves the engine working.
	if _, err := Trace(ctx, pngBytes, nil); err != nil {
		t.Fatal(err)
	}
}

// White artwork on a transparent ground, with every default: the shape comes out white and
// nothing paints the canvas behind it.
func TestWhiteDiscOnTransparent(t *testing.T) {
	const size = 64
	img := image.NewNRGBA(image.Rect(0, 0, size, size))
	// An anti-aliased disc of radius 20, 4x4 supersampled for its rim.
	for y := range size {
		for x := range size {
			in := 0
			for sy := range 4 {
				for sx := range 4 {
					dx := float64(x) + (float64(sx)+0.5)/4 - 32
					dy := float64(y) + (float64(sy)+0.5)/4 - 32
					if dx*dx+dy*dy <= 20*20 {
						in++
					}
				}
			}
			img.SetNRGBA(x, y, color.NRGBA{255, 255, 255, uint8(in * 255 / 16)})
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	for name, trace := range map[string]func() (*Traced, error){
		"png":  func() (*Traced, error) { return Trace(context.Background(), buf.Bytes(), nil) },
		"rgba": func() (*Traced, error) { return TraceRGBA(context.Background(), img.Pix, size, size, nil) },
	} {
		t.Run(name, func(t *testing.T) {
			tr, err := trace()
			if err != nil {
				t.Fatal(err)
			}
			svg := tr.SVG
			fills := regexp.MustCompile(`fill="([^"]+)"`).FindAllStringSubmatch(svg, -1)
			if len(fills) == 0 {
				t.Fatalf("nothing painted:\n%s", svg)
			}
			for _, f := range fills {
				if c := strings.ToLower(f[1]); c != "#fff" && c != "#ffffff" && c != "white" {
					t.Errorf("a fill that is not white: %s\n%s", f[1], svg)
				}
			}
			if regexp.MustCompile(`<rect[^>]*width="(100%|64)"`).MatchString(svg) {
				t.Errorf("a background rectangle:\n%s", svg)
			}
			if !strings.Contains(svg, "<circle") && !strings.Contains(svg, "<path") && !strings.Contains(svg, "<ellipse") {
				t.Errorf("no shape:\n%s", svg)
			}
		})
	}
}

// Many goroutines on few instances: every call gets the sequential result.
func TestConcurrentCalls(t *testing.T) {
	ctx := context.Background()
	e := cachedEngine(t, Config{MaxInstances: 3})
	inputs := [][]byte{fixture(t, "tiny.png"), fixture(t, "white_on_clear.png")}
	want := make([]string, len(inputs))
	for i, in := range inputs {
		tr, err := e.Trace(ctx, in, nil)
		if err != nil {
			t.Fatal(err)
		}
		want[i] = tr.SVG
	}
	const goroutines, rounds = 12, 3
	var wg sync.WaitGroup
	errs := make(chan error, goroutines*rounds)
	for g := range goroutines {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for r := range rounds {
				i := (g + r) % len(inputs)
				tr, err := e.Trace(ctx, inputs[i], nil)
				if err != nil {
					errs <- err
				} else if tr.SVG != want[i] {
					errs <- errors.New("a concurrent call returned a different SVG")
				}
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
	if n := len(e.idle); n > 3 {
		t.Errorf("%d idle instances with MaxInstances 3", n)
	}
}

// A context that has ended stops a call before it starts; one that ends mid-trace returns
// the context's error at once, in both modes, and the engine keeps working.
func TestContextCancellation(t *testing.T) {
	pngBytes := fixture(t, "tiny.png")
	slow := &Options{MaxDim: Ptr(0), Precision: Ptr(0.01)} // about a second in the module
	for _, interruptible := range []bool{false, true} {
		t.Run(fmt.Sprintf("interruptible=%v", interruptible), func(t *testing.T) {
			e := cachedEngine(t, Config{MaxInstances: 1, Interruptible: interruptible})
			ctx, cancel := context.WithTimeout(context.Background(), time.Millisecond)
			defer cancel()
			<-ctx.Done()
			if _, err := e.Trace(ctx, pngBytes, nil); !errors.Is(err, context.DeadlineExceeded) {
				t.Errorf("expired context: got %v", err)
			}

			ctx, cancel = context.WithCancel(context.Background())
			go func() {
				time.Sleep(50 * time.Millisecond)
				cancel()
			}()
			start := time.Now()
			_, err := e.Trace(ctx, pngBytes, slow)
			took := time.Since(start)
			if err == nil {
				t.Skipf("the trace finished in %v, before the cancellation", took)
			}
			if !errors.Is(err, context.Canceled) {
				t.Fatalf("cancelled mid-trace: got %v", err)
			}
			// The next call waits for the one instance: behind the abandoned trace when it
			// runs on, at once when the context stopped it.
			start2 := time.Now()
			if _, err := e.Trace(context.Background(), pngBytes, nil); err != nil {
				t.Fatalf("after a cancellation: %v", err)
			}
			t.Logf("cancelled call returned after %v; the next call took %v", took, time.Since(start2))
		})
	}
}

// An instance that grew past RetainMemory is not kept.
func TestRetainMemory(t *testing.T) {
	e := cachedEngine(t, Config{MaxInstances: 1, RetainMemory: -1})
	if _, err := e.Trace(context.Background(), fixture(t, "tiny.png"), nil); err != nil {
		t.Fatal(err)
	}
	if n := len(e.idle); n != 0 {
		t.Errorf("%d idle instances with RetainMemory -1", n)
	}
	e2 := cachedEngine(t, Config{MaxInstances: 1})
	if _, err := e2.Trace(context.Background(), fixture(t, "tiny.png"), nil); err != nil {
		t.Fatal(err)
	}
	if n := len(e2.idle); n != 1 {
		t.Errorf("%d idle instances, want the one kept", n)
	}
}

// wazero's compiler and its interpreter give the same SVG. The compiler mis-lowers a
// floating-point select on amd64 (tools/wasm_float_select.py rewrites them away at build
// time); without the rewrite this input comes out differently (1296 bytes either way,
// different hashes). Both run on wazero's deterministic clocks: the interpreter is slow
// enough to reach the tracer's wall-clock caps, which would make its output depend on speed.
func TestCompilerMatchesInterpreter(t *testing.T) {
	if testing.Short() {
		t.Skip("the interpreter takes seconds")
	}
	compiled := cachedEngine(t, Config{MaxInstances: 1, fakeClocks: true})
	e := cachedEngine(t, Config{Interpreter: true, MaxInstances: 1, fakeClocks: true})
	for _, c := range []struct {
		input, options string
	}{
		{"tiny.png", `{"max_dim": 24}`},
	} {
		want, err := compiled.TraceJSON(context.Background(), fixture(t, c.input), c.options)
		if err != nil {
			t.Fatal(err)
		}
		got, err := e.TraceJSON(context.Background(), fixture(t, c.input), c.options)
		if err != nil {
			t.Fatal(err)
		}
		if got.SVG != want.SVG {
			t.Errorf("%s %s: the interpreter's SVG (%d bytes) differs from the compiler's (%d bytes)",
				c.input, c.options, len(got.SVG), len(want.SVG))
		}
	}
}
