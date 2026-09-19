// Package inkvec traces raster logos, icons and illustrations to compact, accurate SVG.
//
// It is the Inkvec tracer (https://github.com/logolabs/inkvec), a Rust library, compiled to
// WebAssembly and run by wazero: pure Go, no cgo, no C toolchain, no native library to
// install. The same module runs on every platform Go does, so the output does not depend on
// the host (see [BuildTarget]).
//
//	png, _ := os.ReadFile("logo.png")
//	traced, err := inkvec.Trace(ctx, png, &inkvec.Options{Colors: inkvec.Ptr(16)})
//	if err != nil { ... }
//	os.WriteFile("logo.svg", []byte(traced.SVG), 0o644)
//
// # Options
//
// [Options] is generated from the Rust library's option schema; every field is a pointer and
// nil takes the default. The options cross into the tracer as one JSON object, which Rust
// validates in full: an unknown name, a wrong type or a value out of range is an
// [ErrInvalidOptions]. [TraceJSON] passes a JSON object through untouched, so an option added
// to the tracer works before this package's struct is regenerated. [OptionsSchema] and
// [Defaults] return the schema and the defaults as JSON.
//
// # Performance
//
// The module is single-threaded: WebAssembly outside a browser has no threads here, so one
// trace uses one core, where the native command line spreads the work over all of them.
// Separate calls run in parallel, each on its own module instance (see [Engine]). The first
// call compiles the module, which takes about a second; [Config.CompilationCacheDir] keeps
// the compiled code on disk for the next process.
//
// # Errors
//
// Every error from a trace is an [*Error] and matches one of [ErrInvalidImage],
// [ErrInvalidOptions] or [ErrInternal] with [errors.Is]; a cancelled or expired context
// returns the context's error instead.
package inkvec

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
)

// Traced is a finished trace.
type Traced struct {
	// SVG is the SVG document.
	SVG string
	// Width is the width of the input image in pixels. The SVG's width attribute carries
	// the same number unless the margin option grew it; its viewBox can be a smaller
	// coordinate space when the input was reduced for tracing (the max_dim option).
	Width int
	// Height is the height of the input image in pixels.
	Height int
}

// The kinds of error a trace can fail with. Test for them with [errors.Is]; the error itself
// is an [*Error] carrying the tracer's message.
var (
	// ErrInvalidImage: the bytes are not an image the tracer can decode (PNG, JPEG, WebP, GIF,
	// BMP or TIFF), or raw pixels do not match the size given.
	ErrInvalidImage = errors.New("inkvec: invalid image")
	// ErrInvalidOptions: an option is unknown, of the wrong type or out of range, or the
	// options are not a JSON object.
	ErrInvalidOptions = errors.New("inkvec: invalid options")
	// ErrInternal: the tracer failed or panicked, or the WebAssembly module could not be
	// loaded or ran out of memory. Not the caller's fault; worth a bug report.
	ErrInternal = errors.New("inkvec: internal error")
)

// Error is a failed trace.
type Error struct {
	// Code names the kind of error as the cross-language contract does: "invalid_image",
	// "invalid_options" or "internal".
	Code string
	// Message is the tracer's explanation, for example
	// "invalid options: unknown field `colours`, expected one of ...".
	Message string
}

func (e *Error) Error() string { return "inkvec: " + e.Message }

// Is reports whether target is the sentinel for this error's kind.
func (e *Error) Is(target error) bool {
	switch e.Code {
	case codeInvalidImage:
		return target == ErrInvalidImage
	case codeInvalidOptions:
		return target == ErrInvalidOptions
	default:
		return target == ErrInternal
	}
}

const (
	codeInvalidImage   = "invalid_image"
	codeInvalidOptions = "invalid_options"
	codeInternal       = "internal"
)

func internalError(msg string) *Error { return &Error{Code: codeInternal, Message: msg} }

// Ptr returns a pointer to v, for setting an [Options] field: inkvec.Ptr(16), inkvec.Ptr(true).
func Ptr[T any](v T) *T { return &v }

// optionsJSON marshals opts for the tracer; nil means the defaults.
func optionsJSON(opts *Options) (*string, error) {
	if opts == nil {
		return nil, nil
	}
	b, err := json.Marshal(opts)
	if err != nil {
		// Only a NaN or an infinity gets here; the tracer would refuse it as well.
		return nil, &Error{Code: codeInvalidOptions, Message: "invalid options: " + err.Error()}
	}
	s := string(b)
	return &s, nil
}

// Trace traces an encoded image -- the bytes of a PNG, JPEG, WebP, GIF, BMP or TIFF file --
// to SVG. opts may be nil for the defaults. JPEG and lossy WebP are traced with the
// noise-aware intake the command line uses for them.
//
// It runs on a shared [Engine] created on first use.
func Trace(ctx context.Context, image []byte, opts *Options) (*Traced, error) {
	e, err := defaultEngine()
	if err != nil {
		return nil, err
	}
	return e.Trace(ctx, image, opts)
}

// TraceRGBA traces raw pixels: straight (not premultiplied) RGBA, 8 bits per channel,
// row-major, tightly packed -- exactly width*height*4 bytes, the layout of
// [image.RGBA.Pix] and [image.NRGBA.Pix] when the stride is width*4 (NRGBA is straight
// alpha; RGBA is premultiplied, which only matters for translucent pixels). The SVG is
// byte-identical to [Trace] on a PNG holding the same pixels.
func TraceRGBA(ctx context.Context, pixels []byte, width, height int, opts *Options) (*Traced, error) {
	e, err := defaultEngine()
	if err != nil {
		return nil, err
	}
	return e.TraceRGBA(ctx, pixels, width, height, opts)
}

// TraceJSON is [Trace] with the options as a JSON object, passed to the tracer untouched.
// "" means the defaults. Any option the tracer knows works here, including one added after
// [Options] was generated.
func TraceJSON(ctx context.Context, image []byte, optionsJSON string) (*Traced, error) {
	e, err := defaultEngine()
	if err != nil {
		return nil, err
	}
	return e.TraceJSON(ctx, image, optionsJSON)
}

// TraceRGBAJSON is [TraceRGBA] with the options as a JSON object, as for [TraceJSON].
func TraceRGBAJSON(ctx context.Context, pixels []byte, width, height int, optionsJSON string) (*Traced, error) {
	e, err := defaultEngine()
	if err != nil {
		return nil, err
	}
	return e.TraceRGBAJSON(ctx, pixels, width, height, optionsJSON)
}

// OptionsSchema returns the JSON Schema (draft 2020-12) of the options object: every
// option's name, type, default, description and range, as the tracer defines them.
// It returns "" only if the module could not be loaded; [Trace] then reports why.
func OptionsSchema() string { return defaultInfo().schema }

// Defaults returns every option at its default, as a compact JSON object. Unmarshal it into
// an [Options] or a map. It returns "" only if the module could not be loaded.
func Defaults() string { return defaultInfo().defaults }

// Version returns the tracer's version, e.g. "0.1.3" -- the version of the Rust workspace
// the embedded module was built from, which is also this Go module's release tag.
// It returns "" only if the module could not be loaded.
func Version() string { return defaultInfo().version }

// BuildTarget names the target the tracer was compiled for: "wasm32-wasip1" for this
// package, whatever the host platform. Output is byte-identical between builds with the
// same target; the native libraries (x86_64-linux-gnu, ...) and the browser build
// (wasm32-unknown) can write the same drawing slightly differently, because their maths
// libraries round differently in the last bit.
// It returns "" only if the module could not be loaded.
func BuildTarget() string { return defaultInfo().target }

var (
	defaultOnce sync.Once
	defaultEng  *Engine
	defaultErr  error
)

// defaultEngine is the shared engine behind the package-level functions, created on first
// use with the default [Config] and never closed.
func defaultEngine() (*Engine, error) {
	defaultOnce.Do(func() {
		defaultEng, defaultErr = NewEngine(context.Background(), nil)
	})
	return defaultEng, defaultErr
}

func defaultInfo() moduleInfo {
	e, err := defaultEngine()
	if err != nil {
		return moduleInfo{}
	}
	return e.info
}
