package inkvec

// The cross-language contract, bindings/contract/cases.json, checked the way
// crates/inkvec/tests/contract.rs checks it:
//
//   - expect: the reported width and height, or the kind of error;
//   - svg[target]: the SVG's length and SHA-256 on this build target (BuildTarget(),
//     "wasm32-wasip1"). A target without hashes is reported, not failed, unless
//     INKVEC_CONTRACT_REQUIRE_HASH=1; INKVEC_BLESS=add records them;
//   - same_svg_as: identical SVG to the named case.
//
// The fixtures are found through INKVEC_CONTRACT_DIR, testdata/contract (the mirror
// repository) or ../../bindings/contract (the monorepo); without them the test is skipped.

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type contractCase struct {
	Name      string          `json:"name"`
	Input     string          `json:"input"`
	Form      string          `json:"form"`
	Width     int             `json:"width"`
	Height    int             `json:"height"`
	Options   json.RawMessage `json:"options"`
	SameSVGAs string          `json:"same_svg_as"`
	Expect    struct {
		Width  int    `json:"width"`
		Height int    `json:"height"`
		Error  string `json:"error"`
	} `json:"expect"`
	SVG map[string]svgDigest `json:"svg"`
}

type svgDigest struct {
	Bytes  int    `json:"bytes"`
	SHA256 string `json:"sha256"`
}

func digestOf(svg string) svgDigest {
	sum := sha256.Sum256([]byte(svg))
	return svgDigest{Bytes: len(svg), SHA256: hex.EncodeToString(sum[:])}
}

// contractDir finds the contract fixtures, or skips the test.
func contractDir(t testing.TB) string {
	t.Helper()
	for _, dir := range []string{os.Getenv("INKVEC_CONTRACT_DIR"), "testdata/contract", "../../bindings/contract"} {
		if dir == "" {
			continue
		}
		if _, err := os.Stat(filepath.Join(dir, "cases.json")); err == nil {
			return dir
		}
	}
	t.Skip("contract fixtures not found (set INKVEC_CONTRACT_DIR)")
	return ""
}

func readContract(t testing.TB) (string, []contractCase) {
	t.Helper()
	dir := contractDir(t)
	raw, err := os.ReadFile(filepath.Join(dir, "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		Cases []contractCase `json:"cases"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	return dir, doc.Cases
}

// runCase traces one case with the options passed through as JSON, as every binding does.
func runCase(ctx context.Context, e *Engine, dir string, c contractCase) (*Traced, error) {
	input, err := os.ReadFile(filepath.Join(dir, c.Input))
	if err != nil {
		return nil, err
	}
	if c.Form == "rgba" {
		return e.TraceRGBAJSON(ctx, input, c.Width, c.Height, string(c.Options))
	}
	return e.TraceJSON(ctx, input, string(c.Options))
}

func TestContract(t *testing.T) {
	ctx := context.Background()
	dir, cases := readContract(t)
	e := testEngine(t)
	target := e.BuildTarget()
	bless := os.Getenv("INKVEC_BLESS") == "add"
	requireHash := os.Getenv("INKVEC_CONTRACT_REQUIRE_HASH") == "1"

	svgs := map[string]string{}
	digests := map[string]svgDigest{}
	var unhashed []string
	for _, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			got, err := runCase(ctx, e, dir, c)
			if c.Expect.Error != "" {
				var ie *Error
				if !errors.As(err, &ie) {
					t.Fatalf("want a %s error, got %v", c.Expect.Error, err)
				}
				if ie.Code != c.Expect.Error {
					t.Fatalf("want %s, got %s: %s", c.Expect.Error, ie.Code, ie.Message)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if got.Width != c.Expect.Width || got.Height != c.Expect.Height {
				t.Errorf("size %dx%d, want %dx%d", got.Width, got.Height, c.Expect.Width, c.Expect.Height)
			}
			svgs[c.Name] = got.SVG
			d := digestOf(got.SVG)
			digests[c.Name] = d
			if want, ok := c.SVG[target]; ok && !bless {
				if want != d {
					t.Errorf("SVG on %s: %+v, want %+v", target, d, want)
				}
			} else if !ok {
				unhashed = append(unhashed, fmt.Sprintf("%s: %+v", c.Name, d))
			}
		})
	}
	t.Run("same_svg_as", func(t *testing.T) {
		for _, c := range cases {
			if c.SameSVGAs == "" {
				continue
			}
			other, ok := svgs[c.SameSVGAs]
			if !ok {
				t.Errorf("%s produced no SVG", c.SameSVGAs)
			} else if svgs[c.Name] != other {
				t.Errorf("%s differs from %s", c.Name, c.SameSVGAs)
			}
		}
	})

	switch {
	case bless:
		blessContract(t, filepath.Join(dir, "cases.json"), target, digests)
	case len(unhashed) > 0:
		msg := fmt.Sprintf("no SVG hashes recorded for %s; checked everything else. Record them with INKVEC_BLESS=add:\n  %s",
			target, strings.Join(unhashed, "\n  "))
		if requireHash {
			t.Error(msg)
		} else {
			t.Log(msg)
		}
	}
}

// blessContract records this target's digests in cases.json, keeping every other byte of the
// file -- key order, the other targets, the formatting serde_json gives it -- as it was.
func blessContract(t *testing.T, path, target string, digests map[string]svgDigest) {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	doc, err := decodeOrdered(dec)
	if err != nil {
		t.Fatal(err)
	}
	cases, _ := doc.(*orderedObject).get("cases").([]any)
	for _, c := range cases {
		obj := c.(*orderedObject)
		name, _ := obj.get("name").(string)
		d, ok := digests[name]
		if !ok {
			continue // an error case
		}
		svg, _ := obj.get("svg").(*orderedObject)
		if svg == nil {
			svg = &orderedObject{}
			obj.set("svg", svg)
		}
		svg.set(target, &orderedObject{
			keys: []string{"bytes", "sha256"},
			vals: map[string]any{"bytes": json.Number(fmt.Sprint(d.Bytes)), "sha256": d.SHA256},
		})
	}
	var out bytes.Buffer
	writeOrdered(&out, doc, "")
	out.WriteByte('\n')
	if err := os.WriteFile(path, out.Bytes(), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Logf("recorded %s hashes for %d cases in %s", target, len(digests), path)
}

// orderedObject is a JSON object that remembers its key order.
type orderedObject struct {
	keys []string
	vals map[string]any
}

func (o *orderedObject) get(k string) any { return o.vals[k] }

func (o *orderedObject) set(k string, v any) {
	if o.vals == nil {
		o.vals = map[string]any{}
	}
	if _, ok := o.vals[k]; !ok {
		o.keys = append(o.keys, k)
	}
	o.vals[k] = v
}

func decodeOrdered(dec *json.Decoder) (any, error) {
	tok, err := dec.Token()
	if err != nil {
		return nil, err
	}
	switch v := tok.(type) {
	case json.Delim:
		switch v {
		case '{':
			o := &orderedObject{}
			for dec.More() {
				kt, err := dec.Token()
				if err != nil {
					return nil, err
				}
				val, err := decodeOrdered(dec)
				if err != nil {
					return nil, err
				}
				o.set(kt.(string), val)
			}
			_, err := dec.Token()
			return o, err
		case '[':
			arr := []any{}
			for dec.More() {
				val, err := decodeOrdered(dec)
				if err != nil {
					return nil, err
				}
				arr = append(arr, val)
			}
			_, err := dec.Token()
			return arr, err
		}
		return nil, fmt.Errorf("unexpected %v", v)
	default:
		return v, nil
	}
}

// writeOrdered writes v the way serde_json's pretty printer does: two-space indent,
// "key": value, empty containers as {} and [].
func writeOrdered(w *bytes.Buffer, v any, indent string) {
	inner := indent + "  "
	switch v := v.(type) {
	case *orderedObject:
		if len(v.keys) == 0 {
			w.WriteString("{}")
			return
		}
		w.WriteString("{\n")
		for i, k := range v.keys {
			w.WriteString(inner)
			writeString(w, k)
			w.WriteString(": ")
			writeOrdered(w, v.vals[k], inner)
			if i < len(v.keys)-1 {
				w.WriteByte(',')
			}
			w.WriteByte('\n')
		}
		w.WriteString(indent + "}")
	case []any:
		if len(v) == 0 {
			w.WriteString("[]")
			return
		}
		w.WriteString("[\n")
		for i, x := range v {
			w.WriteString(inner)
			writeOrdered(w, x, inner)
			if i < len(v)-1 {
				w.WriteByte(',')
			}
			w.WriteByte('\n')
		}
		w.WriteString(indent + "]")
	case string:
		writeString(w, v)
	case json.Number:
		w.WriteString(v.String())
	case bool:
		fmt.Fprint(w, v)
	case nil:
		w.WriteString("null")
	default:
		panic(fmt.Sprintf("unexpected JSON value %T", v))
	}
}

func writeString(w *bytes.Buffer, s string) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(s)
	w.Write(bytes.TrimRight(b.Bytes(), "\n"))
}
