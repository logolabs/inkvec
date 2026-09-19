package inkvec

import (
	"bytes"
	"context"
	"fmt"
	"math"
	"runtime"
	"strings"
	"sync"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
	"github.com/tetratelabs/wazero/imports/wasi_snapshot_preview1"
)

// Config tunes an [Engine]. The zero value is the default.
type Config struct {
	// MaxInstances bounds the module instances alive at once. Each runs one trace at a time
	// on one core with its own memory, so this bounds both the parallelism and the memory;
	// calls beyond it wait for a free instance. 0 means runtime.GOMAXPROCS(0).
	MaxInstances int

	// RetainMemory is the largest linear memory, in bytes, an instance may have grown to and
	// still be kept for the next call. WebAssembly memory never shrinks, so an instance that
	// traced a large image is closed after the call instead, and its memory returns to Go.
	// 0 means 256 MiB; a negative value closes every instance after its call.
	RetainMemory int64

	// CompilationCacheDir, when set, is a directory where wazero keeps the compiled module,
	// so a later process skips the compilation. The directory is created if missing, and is
	// safe to share between processes.
	CompilationCacheDir string

	// Interruptible makes a cancelled or expired context stop a trace mid-way. It compiles
	// the module with wazero's termination checks (WithCloseOnContextDone), which made
	// tracing about 3.5 times slower in this package's measurements, so it is off by default.
	// Off, a call still returns as soon as its context ends, and the abandoned trace runs to
	// completion in the background, holding its instance (and its MaxInstances slot) until
	// then.
	Interruptible bool

	// Interpreter runs the module in wazero's interpreter even where its compiler is
	// available. Far slower; wazero picks the interpreter by itself on platforms its
	// compiler does not support.
	Interpreter bool

	// fakeClocks gives the module wazero's deterministic clocks (1 ms per reading) instead of
	// the system's. For tests: the tracer has wall-clock caps on two stages, which a slow
	// engine (the interpreter) can reach, and then its output depends on speed.
	fakeClocks bool
}

const defaultRetainMemory = 256 << 20

// Engine runs the tracer. It compiles the WebAssembly module once and keeps a pool of
// module instances; calls may be made from any number of goroutines at once.
//
// The package-level functions use a shared Engine with the default [Config]. Create one
// yourself to choose the configuration, or to release its memory with [Engine.Close].
type Engine struct {
	rt            wazero.Runtime
	compiled      wazero.CompiledModule
	slots         chan struct{}  // one token per live instance
	idle          chan *instance // instances ready for a call
	retain        int64
	interruptible bool
	fakeClocks    bool
	info          moduleInfo
}

// moduleInfo is what the module says about itself; read once.
type moduleInfo struct {
	version, target, schema, defaults string
}

// abiVersion is the C ABI this package speaks (INKVEC_ABI_VERSION).
const abiVersion = 1

// The C ABI's status codes (INKVEC_OK, INKVEC_ERR_*).
const (
	statusOK              = 0
	statusInvalidArgument = 1
	statusInvalidImage    = 2
	statusInvalidOptions  = 3
)

// InkvecResult on wasm32: struct_size u32, status i32, svg ptr, svg_len usize, width u32,
// height u32, error ptr -- seven 4-byte fields.
const (
	resultSize = 28
	offStatus  = 4
	offSVG     = 8
	offSVGLen  = 12
	offWidth   = 16
	offHeight  = 20
	offError   = 24
)

// NewEngine compiles the module and checks that it speaks this package's ABI. cfg may be
// nil for the defaults.
func NewEngine(ctx context.Context, cfg *Config) (*Engine, error) {
	if cfg == nil {
		cfg = &Config{}
	}
	var rc wazero.RuntimeConfig
	if cfg.Interpreter {
		rc = wazero.NewRuntimeConfigInterpreter()
	} else {
		rc = wazero.NewRuntimeConfig() // the compiler where supported, else the interpreter
	}
	rc = rc.WithCloseOnContextDone(cfg.Interruptible)
	if cfg.CompilationCacheDir != "" {
		cache, err := wazero.NewCompilationCacheWithDir(cfg.CompilationCacheDir)
		if err != nil {
			return nil, internalError("compilation cache: " + err.Error())
		}
		rc = rc.WithCompilationCache(cache)
	}
	rt := wazero.NewRuntimeWithConfig(ctx, rc)
	if _, err := wasi_snapshot_preview1.Instantiate(ctx, rt); err != nil {
		_ = rt.Close(ctx)
		return nil, internalError("WASI: " + err.Error())
	}
	compileCtx := experimental.WithCompilationWorkers(ctx, runtime.GOMAXPROCS(0))
	compiled, err := rt.CompileModule(compileCtx, wasmModule)
	if err != nil {
		_ = rt.Close(ctx)
		return nil, internalError("compiling the WebAssembly module: " + err.Error())
	}
	n := cfg.MaxInstances
	if n <= 0 {
		n = runtime.GOMAXPROCS(0)
	}
	retain := cfg.RetainMemory
	if retain == 0 {
		retain = defaultRetainMemory
	}
	e := &Engine{
		rt:            rt,
		compiled:      compiled,
		slots:         make(chan struct{}, n),
		idle:          make(chan *instance, n),
		retain:        retain,
		interruptible: cfg.Interruptible,
		fakeClocks:    cfg.fakeClocks,
	}
	if err := e.readInfo(ctx); err != nil {
		_ = rt.Close(ctx)
		return nil, err
	}
	return e, nil
}

// readInfo checks the ABI version and reads the static strings, on one instance that then
// joins the pool.
func (e *Engine) readInfo(ctx context.Context) error {
	in, err := e.acquire(ctx)
	if err != nil {
		return err
	}
	ok := false
	defer func() { e.release(in, ok) }()
	abi, err := in.call(ctx, "inkvec_abi_version")
	if err != nil {
		return err
	}
	if abi != abiVersion {
		return internalError(fmt.Sprintf("the module speaks C ABI %d, this package %d", abi, abiVersion))
	}
	for _, s := range []struct {
		fn  string
		dst *string
	}{
		{"inkvec_version", &e.info.version},
		{"inkvec_build_target", &e.info.target},
		{"inkvec_options_schema", &e.info.schema},
		{"inkvec_default_options", &e.info.defaults},
	} {
		p, err := in.call(ctx, s.fn)
		if err != nil {
			return err
		}
		if *s.dst, err = in.cString(p); err != nil {
			return err
		}
	}
	ok = true
	return nil
}

// Close releases the engine: its compiled code and every instance. Calls must not be in
// flight; a call made afterwards fails.
func (e *Engine) Close(ctx context.Context) error {
	for {
		select {
		case in := <-e.idle:
			in.close(ctx)
		default:
			return e.rt.Close(ctx)
		}
	}
}

// Trace is the package-level [Trace] on this engine.
func (e *Engine) Trace(ctx context.Context, image []byte, opts *Options) (*Traced, error) {
	o, err := optionsJSON(opts)
	if err != nil {
		return nil, err
	}
	return e.run(ctx, image, false, 0, 0, o)
}

// TraceRGBA is the package-level [TraceRGBA] on this engine.
func (e *Engine) TraceRGBA(ctx context.Context, pixels []byte, width, height int, opts *Options) (*Traced, error) {
	o, err := optionsJSON(opts)
	if err != nil {
		return nil, err
	}
	w, h, err := dims(width, height)
	if err != nil {
		return nil, err
	}
	return e.run(ctx, pixels, true, w, h, o)
}

// TraceJSON is the package-level [TraceJSON] on this engine.
func (e *Engine) TraceJSON(ctx context.Context, image []byte, optionsJSON string) (*Traced, error) {
	return e.run(ctx, image, false, 0, 0, &optionsJSON)
}

// TraceRGBAJSON is the package-level [TraceRGBAJSON] on this engine.
func (e *Engine) TraceRGBAJSON(ctx context.Context, pixels []byte, width, height int, optionsJSON string) (*Traced, error) {
	w, h, err := dims(width, height)
	if err != nil {
		return nil, err
	}
	return e.run(ctx, pixels, true, w, h, &optionsJSON)
}

// OptionsSchema is the package-level [OptionsSchema].
func (e *Engine) OptionsSchema() string { return e.info.schema }

// Defaults is the package-level [Defaults].
func (e *Engine) Defaults() string { return e.info.defaults }

// Version is the package-level [Version].
func (e *Engine) Version() string { return e.info.version }

// BuildTarget is the package-level [BuildTarget].
func (e *Engine) BuildTarget() string { return e.info.target }

// dims checks that a size fits the C ABI's uint32; the tracer checks the rest.
func dims(width, height int) (uint32, uint32, error) {
	if width < 0 || height < 0 || uint64(width) > math.MaxUint32 || uint64(height) > math.MaxUint32 {
		return 0, 0, &Error{Code: codeInvalidImage, Message: fmt.Sprintf("invalid image: %dx%d is not a size", width, height)}
	}
	return uint32(width), uint32(height), nil
}

type outcome struct {
	t   *Traced
	err error
}

// run is one trace: take an instance, copy the input in, call the C ABI, give the instance
// back.
func (e *Engine) run(ctx context.Context, input []byte, rgba bool, w, h uint32, opts *string) (*Traced, error) {
	if opts != nil && strings.IndexByte(*opts, 0) >= 0 {
		return nil, &Error{Code: codeInvalidOptions, Message: "invalid options: the JSON holds a NUL byte"}
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	in, err := e.acquire(ctx)
	if err != nil {
		return nil, err
	}
	// Without wazero's termination checks, a context cannot stop the module; the calls
	// get one that never ends, and the caller is released below instead.
	callCtx := ctx
	if !e.interruptible {
		callCtx = context.WithoutCancel(ctx)
	}
	// The input is copied in before this function can return, so the caller's slices are
	// never read after it.
	j, err := in.prepare(callCtx, input, opts)
	if err != nil {
		e.release(in, false)
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		return nil, err
	}
	finish := func() outcome {
		t, healthy, err := in.finish(callCtx, j, rgba, w, h)
		e.release(in, healthy)
		if err != nil && !healthy && ctx.Err() != nil {
			return outcome{nil, ctx.Err()} // stopped by the context
		}
		return outcome{t, err}
	}
	if e.interruptible || ctx.Done() == nil {
		o := finish()
		return o.t, o.err
	}
	done := make(chan outcome, 1)
	go func() { done <- finish() }()
	select {
	case o := <-done:
		return o.t, o.err
	case <-ctx.Done():
		return nil, ctx.Err() // the trace finishes in the background and frees its instance
	}
}

// acquire takes an idle instance or, below MaxInstances, starts one; otherwise it waits.
func (e *Engine) acquire(ctx context.Context) (*instance, error) {
	select {
	case e.slots <- struct{}{}:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	select {
	case in := <-e.idle:
		return in, nil
	default:
	}
	in, err := e.instantiate(ctx)
	if err != nil {
		<-e.slots
		return nil, err
	}
	return in, nil
}

// release returns an instance to the pool, or closes it when it is broken or has grown past
// RetainMemory.
func (e *Engine) release(in *instance, healthy bool) {
	if healthy && e.retain > 0 && int64(in.mem.Size()) <= e.retain {
		e.idle <- in // never blocks: at most cap(slots) instances exist
	} else {
		in.close(context.Background())
	}
	<-e.slots
}

// instance is one instantiation of the module: its own memory, one call at a time.
type instance struct {
	mod    api.Module
	mem    api.Memory
	stderr *limitedBuffer
	fns    map[string]api.Function
}

var exports = []string{
	"inkvec_trace", "inkvec_trace_rgba", "inkvec_result_free", "inkvec_alloc", "inkvec_dealloc",
	"inkvec_abi_version", "inkvec_version", "inkvec_build_target", "inkvec_options_schema",
	"inkvec_default_options",
}

func (e *Engine) instantiate(ctx context.Context) (*instance, error) {
	stderr := &limitedBuffer{max: 4096}
	cfg := wazero.NewModuleConfig().
		// Anonymous, so the module can be instantiated any number of times.
		WithName("").
		// A library, not a command: run a reactor's initialiser if the toolchain ever adds
		// one, never `_start`.
		WithStartFunctions("_initialize").
		// No environment: the tracer's INKVEC_* research switches stay off whatever this
		// process sets, and the output stays reproducible.
		// A panic's message, for the error that reports it.
		WithStderr(stderr)
	if !e.fakeClocks {
		// Real clocks, so the time_budget option measures time.
		cfg = cfg.WithSysWalltime().WithSysNanotime().WithSysNanosleep()
	}
	mod, err := e.rt.InstantiateModule(ctx, e.compiled, cfg)
	if err != nil {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		return nil, internalError("instantiating the WebAssembly module: " + err.Error())
	}
	in := &instance{mod: mod, mem: mod.Memory(), stderr: stderr, fns: map[string]api.Function{}}
	for _, name := range exports {
		fn := mod.ExportedFunction(name)
		if fn == nil {
			_ = mod.Close(ctx)
			return nil, internalError("the WebAssembly module does not export " + name)
		}
		in.fns[name] = fn
	}
	if in.mem == nil {
		_ = mod.Close(ctx)
		return nil, internalError("the WebAssembly module exports no memory")
	}
	return in, nil
}

func (in *instance) close(ctx context.Context) { _ = in.mod.Close(ctx) }

// call runs an exported function and returns its first result as a uint32 (every result in
// this ABI is an i32). An error means the instance must not be reused: the tracer trapped
// (a Rust panic aborts), or the context stopped it.
func (in *instance) call(ctx context.Context, name string, args ...uint64) (uint32, error) {
	res, err := in.fns[name].Call(ctx, args...)
	if err != nil {
		if ctx.Err() != nil {
			return 0, ctx.Err()
		}
		msg := "the tracer stopped: " + err.Error()
		if s := strings.TrimSpace(in.stderr.String()); s != "" {
			msg = "the tracer stopped: " + s + " (" + firstLine(err.Error()) + ")"
		}
		return 0, internalError(msg)
	}
	if len(res) == 0 {
		return 0, nil
	}
	return uint32(res[0]), nil
}

// buffer is an allocation in the module's memory.
type buffer struct{ ptr, size uint32 }

// alloc places b, plus a NUL when nul is set, in the module's memory. The buffer is never
// empty, so an empty input still gets a valid pointer (and the tracer says why the bytes are
// not an image).
func (in *instance) alloc(ctx context.Context, b []byte, nul bool) (buffer, error) {
	n := uint64(len(b))
	if nul {
		n++
	}
	n = max(n, 1)
	if n > math.MaxUint32-64 {
		return buffer{}, internalError(fmt.Sprintf("%d bytes do not fit in a 32-bit WebAssembly memory", len(b)))
	}
	p, err := in.call(ctx, "inkvec_alloc", n)
	if err != nil {
		return buffer{}, err
	}
	if p == 0 {
		return buffer{}, internalError(fmt.Sprintf("the WebAssembly module could not allocate %d bytes", n))
	}
	if !in.mem.Write(p, b) || (nul && !in.mem.WriteByte(p+uint32(len(b)), 0)) {
		return buffer{}, internalError("write outside the WebAssembly memory")
	}
	return buffer{p, uint32(n)}, nil
}

// job is a trace whose input, options and result struct are in the module's memory.
type job struct {
	input, options, result buffer
	inputLen               int
	hasOptions             bool
}

// prepare copies the input and the options into the instance. On error the instance must be
// discarded.
func (in *instance) prepare(ctx context.Context, input []byte, opts *string) (j job, err error) {
	in.stderr.Reset()
	j.inputLen = len(input)
	if j.input, err = in.alloc(ctx, input, false); err != nil {
		return j, err
	}
	if opts != nil {
		j.hasOptions = true
		if j.options, err = in.alloc(ctx, []byte(*opts), true); err != nil {
			return j, err
		}
	}
	var header [resultSize]byte
	header[0] = resultSize // struct_size, a little-endian u32
	j.result, err = in.alloc(ctx, header[:], false)
	return j, err
}

// finish runs a prepared trace and releases its buffers. healthy is false when the instance
// must be discarded.
func (in *instance) finish(ctx context.Context, j job, rgba bool, w, h uint32) (t *Traced, healthy bool, err error) {
	var optPtr uint32
	if j.hasOptions {
		optPtr = j.options.ptr
	}
	var status uint32
	if rgba {
		status, err = in.call(ctx, "inkvec_trace_rgba", uint64(j.input.ptr), uint64(j.inputLen),
			uint64(w), uint64(h), uint64(optPtr), uint64(j.result.ptr))
	} else {
		status, err = in.call(ctx, "inkvec_trace", uint64(j.input.ptr), uint64(j.inputLen),
			uint64(optPtr), uint64(j.result.ptr))
	}
	if err != nil {
		return nil, false, err
	}

	res, ok := in.mem.Read(j.result.ptr, resultSize)
	if !ok {
		return nil, false, internalError("result outside the WebAssembly memory")
	}
	u32 := func(off int) uint32 {
		return uint32(res[off]) | uint32(res[off+1])<<8 | uint32(res[off+2])<<16 | uint32(res[off+3])<<24
	}
	if stored := u32(offStatus); stored != status {
		return nil, false, internalError(fmt.Sprintf("status %d returned but %d stored", status, stored))
	}
	if status == statusOK {
		svg, ok := in.mem.Read(u32(offSVG), u32(offSVGLen))
		if !ok {
			return nil, false, internalError("SVG outside the WebAssembly memory")
		}
		t = &Traced{SVG: string(svg), Width: int(u32(offWidth)), Height: int(u32(offHeight))}
	} else {
		msg, cerr := in.cString(u32(offError))
		if cerr != nil {
			return nil, false, cerr
		}
		err = statusError(status, msg)
	}
	if _, ferr := in.call(ctx, "inkvec_result_free", uint64(j.result.ptr)); ferr != nil {
		return nil, false, ferr
	}
	for _, b := range []buffer{j.input, j.options, j.result} {
		if b.ptr == 0 {
			continue
		}
		if _, derr := in.call(ctx, "inkvec_dealloc", uint64(b.ptr), uint64(b.size)); derr != nil {
			return t, false, err // the result stands; the instance does not
		}
	}
	return t, true, err
}

// statusError is the Go error for a C ABI status and message.
func statusError(status uint32, msg string) error {
	switch status {
	case statusInvalidImage:
		return &Error{Code: codeInvalidImage, Message: msg}
	case statusInvalidOptions:
		return &Error{Code: codeInvalidOptions, Message: msg}
	case statusInvalidArgument:
		return internalError("the binding passed an invalid argument: " + msg)
	default:
		return internalError(msg)
	}
}

// cString reads a NUL-terminated string at p.
func (in *instance) cString(p uint32) (string, error) {
	if p == 0 {
		return "", internalError("NULL string from the WebAssembly module")
	}
	size := in.mem.Size()
	if p >= size {
		return "", internalError("string outside the WebAssembly memory")
	}
	b, _ := in.mem.Read(p, size-p)
	end := bytes.IndexByte(b, 0)
	if end < 0 {
		return "", internalError("unterminated string from the WebAssembly module")
	}
	return string(b[:end]), nil
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}

// limitedBuffer keeps the first max bytes written to it: the module's stderr, which holds a
// panic's message when the tracer aborts.
type limitedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
	max int
}

func (b *limitedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if room := b.max - b.buf.Len(); room > 0 {
		b.buf.Write(p[:min(room, len(p))])
	}
	return len(p), nil
}

func (b *limitedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

func (b *limitedBuffer) Reset() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.buf.Reset()
}
