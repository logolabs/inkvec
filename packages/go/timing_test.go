package inkvec

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"testing"
	"time"
)

// TestTimingVsNative compares this package with the native command line on the web demo's
// sample images. Rough: wall-clock, a few repetitions, whatever else the machine is doing.
// Opt in with the path of a native `inkvec` binary built from the same commit:
//
//	INKVEC_CLI=../../target/release/inkvec go test -run TimingVsNative -v
func TestTimingVsNative(t *testing.T) {
	cli := os.Getenv("INKVEC_CLI")
	if cli == "" {
		t.Skip("set INKVEC_CLI to a native inkvec binary")
	}
	samples, _ := filepath.Glob("../../web/samples/*.png")
	if len(samples) == 0 {
		t.Skip("no web/samples")
	}
	const reps = 3
	ctx := context.Background()

	start := time.Now()
	e, err := NewEngine(ctx, &Config{MaxInstances: 1})
	if err != nil {
		t.Fatal(err)
	}
	defer e.Close(ctx)
	t.Logf("engine: compiled and first instance in %v", time.Since(start).Round(time.Millisecond))

	median := func(d []time.Duration) time.Duration {
		slices.Sort(d)
		return d[len(d)/2]
	}
	out := filepath.Join(t.TempDir(), "out.svg")
	native := func(path string, threads string) time.Duration {
		var ds []time.Duration
		for range reps {
			cmd := exec.Command(cli, path, "-o", out)
			cmd.Env = append(os.Environ(), "RAYON_NUM_THREADS="+threads)
			s := time.Now()
			if b, err := cmd.CombinedOutput(); err != nil {
				t.Fatalf("%s: %v\n%s", cli, err, b)
			}
			ds = append(ds, time.Since(s))
		}
		return median(ds)
	}

	t.Logf("%-24s %8s %10s %10s %10s %8s %8s", "sample", "pixels", "go(wasm)", "native 1t", "native all", "wasm/1t", "memory")
	for _, path := range samples {
		img, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		// A fresh instance per sample, so its memory is this image's peak.
		select {
		case in := <-e.idle:
			in.close(ctx)
		default:
		}
		var ds []time.Duration
		var tr *Traced
		for range reps {
			s := time.Now()
			if tr, err = e.Trace(ctx, img, nil); err != nil {
				t.Fatal(err)
			}
			ds = append(ds, time.Since(s))
		}
		goTime := median(ds)
		one := native(path, "1")
		all := native(path, "0")
		var mem uint32
		select {
		case in := <-e.idle:
			mem = in.mem.Size()
			e.idle <- in
		default:
		}
		t.Logf("%-24s %8s %10v %10v %10v %7.1fx %6dMiB", filepath.Base(path),
			fmt.Sprintf("%dx%d", tr.Width, tr.Height),
			goTime.Round(time.Millisecond), one.Round(time.Millisecond), all.Round(time.Millisecond),
			float64(goTime)/float64(one), mem>>20)
	}
}
