package run

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"testing/iotest"

	"github.com/grpc-ecosystem/grpc-gateway/v2/runtime"
	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/genproto/googleapis/api/httpbody"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoregistry"
)

func browserModelCodecFixture(t testing.TB) ([]byte, *browserModelCodec) {
	t.Helper()
	data, err := readBrowserHTTPModel(os.Getenv("BROWSER_OPENFGA_TEST_MODEL"))
	if err != nil {
		t.Fatal(err)
	}
	codec, err := newBrowserModelCodec(data)
	if err != nil {
		t.Fatal(err)
	}
	return data, codec
}

func assertBrowserModelCodecParity(t *testing.T, codec *browserModelCodec, reader func() io.Reader, steps int) {
	t.Helper()
	standard := codec.JSONPb.NewDecoder(reader())
	candidate := codec.NewDecoder(reader())
	for step := 0; step < steps; step++ {
		initial := &openfgav1.WriteAuthorizationModelRequest{StoreId: "previous", SchemaVersion: "previous"}
		initial.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01})
		want := proto.Clone(initial).(*openfgav1.WriteAuthorizationModelRequest)
		got := proto.Clone(initial).(*openfgav1.WriteAuthorizationModelRequest)
		wantErr, gotErr := standard.Decode(want), candidate.Decode(got)
		if fmt.Sprintf("%T:%v", gotErr, gotErr) != fmt.Sprintf("%T:%v", wantErr, wantErr) {
			t.Fatalf("step %d error parity: got %T:%v; want %T:%v", step, gotErr, gotErr, wantErr, wantErr)
		}
		if !proto.Equal(got, want) {
			t.Fatalf("step %d changed decoded/partially decoded protobuf", step)
		}
	}
}

func TestBrowserHTTPModelCodecExactCopies(t *testing.T) {
	data, codec := browserModelCodecFixture(t)
	output := captureBrowserHTTPWarmupLog(t)
	want := &openfgav1.WriteAuthorizationModelRequest{}
	if err := codec.JSONPb.NewDecoder(bytes.NewReader(data)).Decode(want); err != nil {
		t.Fatal(err)
	}
	input := bytes.Clone(data)
	for i := range data {
		data[i] = 'x' // Input ownership must not reach the private byte key.
	}
	for _, body := range [][]byte{input, bytes.TrimSpace(input), append([]byte(" \r\n\t"), input...)} {
		got := &openfgav1.WriteAuthorizationModelRequest{StoreId: "previous"}
		got.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01})
		if err := codec.NewDecoder(bytes.NewReader(body)).Decode(got); err != nil {
			t.Fatal(err)
		}
		if !proto.Equal(got, want) || got.GetStoreId() != "" {
			t.Fatal("hit changed model content, kept an old field, or supplied a fixture store")
		}
		got.StoreId = "changed"
		got.TypeDefinitions[0].Type = "changed"
		got.TypeDefinitions[0].Metadata.Module = "changed"
		for _, definition := range got.TypeDefinitions {
			if len(definition.Relations) > 0 {
				for relation := range definition.Relations {
					delete(definition.Relations, relation)
					break
				}
				break
			}
		}
		got.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x02})
	}
	if !proto.Equal(codec.prototype, want) || strings.Count(output.String(), "cache_hit=true") != 3 {
		t.Fatal("returned request aliases prototype, or exact input did not use the bounded fast path")
	}
	if strings.Contains(output.String(), "lakekeeper") || strings.Contains(output.String(), "schema_version") {
		t.Fatal("decode diagnostics disclosed model content")
	}
}

func TestBrowserHTTPModelCodecFallbackParity(t *testing.T) {
	data, codec := browserModelCodecFixture(t)
	raw := bytes.TrimSpace(data)
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil {
		t.Fatal(err)
	}
	changedRewrite := bytes.Replace(raw, []byte(`"this":{}`), []byte(`"computedUserset":{"relation":"missing"}`), 1)
	if bytes.Equal(changedRewrite, raw) {
		t.Fatal("rewrite negative fixture did not change a real relation")
	}
	cases := []struct {
		name string
		body []byte
	}{
		{"internal_whitespace", append([]byte("{ "), raw[1:]...)},
		{"field_order", []byte(`{"type_definitions":` + string(fields["type_definitions"]) + `,"schema_version":"1.2"}`)},
		{"unknown_field", append([]byte(`{"private_probe":"redacted",`), raw[1:]...)},
		{"duplicate_field", append([]byte(`{"schema_version":"1.2",`), raw[1:]...)},
		{"schema", bytes.Replace(raw, []byte(`"1.2"`), []byte(`"1.3"`), 1)},
		{"metadata", bytes.Replace(raw, []byte("components/model_version.fga"), []byte("components/other_version.fga"), 1)},
		{"rewrite", changedRewrite},
		{"conditions", append([]byte(`{"conditions":{"x":{"name":"x","expression":"true"}},`), raw[1:]...)},
		{"body_store_id", append([]byte(`{"store_id":"01J00000000000000000000001",`), raw[1:]...)},
		{"empty_object", []byte("{}")},
		{"null", []byte("null")},
		{"malformed", []byte("{")},
		{"truncated", raw[:len(raw)/2]},
		{"empty", nil},
	}
	output := captureBrowserHTTPWarmupLog(t)
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			output.Reset()
			assertBrowserModelCodecParity(t, codec, func() io.Reader { return bytes.NewReader(tc.body) }, 1)
			if !strings.Contains(output.String(), "cache_hit=false") || strings.Contains(output.String(), "private_probe") {
				t.Fatal("nonidentical body bypassed original decode or diagnostics leaked input")
			}
		})
	}
}

func TestBrowserHTTPModelCodecStreamAndReaderParity(t *testing.T) {
	data, codec := browserModelCodecFixture(t)
	raw := bytes.TrimSpace(data)
	errRead := errors.New("private reader failure")
	for _, tc := range []struct {
		name   string
		reader func() io.Reader
		steps  int
	}{
		{"multiple_values", func() io.Reader { return strings.NewReader(string(raw) + " {} " + string(raw)) }, 4},
		{"trailing_invalid", func() io.Reader { return strings.NewReader(string(raw) + " !") }, 3},
		{"one_byte_reader", func() io.Reader { return iotest.OneByteReader(bytes.NewReader(raw)) }, 2},
		{"data_and_eof", func() io.Reader { return iotest.DataErrReader(bytes.NewReader(raw)) }, 2},
		{"reader_error", func() io.Reader { return iotest.ErrReader(errRead) }, 2},
		{"partial_reader_error", func() io.Reader {
			return io.MultiReader(bytes.NewReader(raw[:len(raw)/2]), iotest.ErrReader(errRead))
		}, 2},
	} {
		t.Run(tc.name, func(t *testing.T) { assertBrowserModelCodecParity(t, codec, tc.reader, tc.steps) })
	}
	// Different destination types must share the same decoder/read-ahead state.
	stream := `{"name":"store"} ` + string(raw) + ` {"key":"value"}`
	standard, candidate := codec.JSONPb.NewDecoder(strings.NewReader(stream)), codec.NewDecoder(strings.NewReader(stream))
	for step, pair := range [][2]interface{}{
		{&openfgav1.CreateStoreRequest{}, &openfgav1.CreateStoreRequest{}},
		{&openfgav1.WriteAuthorizationModelRequest{}, &openfgav1.WriteAuthorizationModelRequest{}},
		{&map[string]string{}, &map[string]string{}},
	} {
		if wantErr, gotErr := standard.Decode(pair[0]), candidate.Decode(pair[1]); wantErr != nil || gotErr != nil {
			t.Fatalf("mixed type step %d: standard %v, candidate %v", step, wantErr, gotErr)
		}
		want, _ := json.Marshal(pair[0])
		got, _ := json.Marshal(pair[1])
		if !bytes.Equal(got, want) {
			t.Fatalf("mixed type step %d differs", step)
		}
	}
}

func TestBrowserHTTPModelCodecOptionsAndOutboundParity(t *testing.T) {
	data, _ := browserModelCodecFixture(t)
	for _, options := range []protojson.UnmarshalOptions{
		{},
		{DiscardUnknown: true, AllowPartial: true},
		{DiscardUnknown: true, Resolver: protoregistry.GlobalTypes},
		{DiscardUnknown: true, RecursionLimit: 1},
	} {
		codec, err := newBrowserModelCodec(data)
		if err != nil {
			t.Fatal(err)
		}
		codec.UnmarshalOptions = options
		if _, fast := codec.NewDecoder(bytes.NewReader(data)).(*browserModelDecoder); fast {
			t.Fatal("non-default unmarshal options incorrectly enabled cached decode")
		}
		assertBrowserModelCodecParity(t, codec, func() io.Reader { return bytes.NewReader(data) }, 1)
	}
	_, codec := browserModelCodecFixture(t)
	codec.MarshalOptions = protojson.MarshalOptions{UseProtoNames: true, Indent: "  "}
	for _, value := range []interface{}{
		&openfgav1.WriteAuthorizationModelResponse{AuthorizationModelId: "01J00000000000000000000001"},
		map[string]string{"key": "value"},
	} {
		want, wantErr := codec.JSONPb.Marshal(value)
		got, gotErr := codec.Marshal(value)
		if !bytes.Equal(got, want) || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
			t.Fatal("outbound Marshal semantics changed")
		}
		var a, b bytes.Buffer
		wantErr, gotErr = codec.JSONPb.NewEncoder(&a).Encode(value), codec.NewEncoder(&b).Encode(value)
		if !bytes.Equal(a.Bytes(), b.Bytes()) || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
			t.Fatal("outbound Encoder semantics changed")
		}
		if codec.ContentType(value) != codec.JSONPb.ContentType(value) || !bytes.Equal(codec.Delimiter(), codec.JSONPb.Delimiter()) {
			t.Fatal("content type or streaming delimiter changed")
		}
	}
	httpBody := &httpbody.HttpBody{ContentType: "application/octet-stream", Data: []byte{0, 1, 2}}
	wrapped := &runtime.HTTPBodyMarshaler{Marshaler: codec}
	got, err := wrapped.Marshal(httpBody)
	if err != nil || !bytes.Equal(got, httpBody.Data) || wrapped.ContentType(httpBody) != httpBody.ContentType {
		t.Fatal("HTTPBody wrapper semantics changed")
	}
}

type browserModelCodecPanicReader struct{ value interface{} }

func (r browserModelCodecPanicReader) Read([]byte) (int, error) { panic(r.value) }

func TestBrowserHTTPModelCodecPreservesPanic(t *testing.T) {
	_, codec := browserModelCodecFixture(t)
	output := captureBrowserHTTPWarmupLog(t)
	token := &struct{ marker string }{"private panic marker"}
	catch := func(decoder runtime.Decoder) (value interface{}) {
		defer func() { value = recover() }()
		_ = decoder.Decode(&openfgav1.WriteAuthorizationModelRequest{})
		return nil
	}
	if got := catch(codec.JSONPb.NewDecoder(browserModelCodecPanicReader{token})); got != token {
		t.Fatal("baseline reader did not panic as expected")
	}
	if got := catch(codec.NewDecoder(browserModelCodecPanicReader{token})); got != token {
		t.Fatal("candidate swallowed or replaced the reader panic")
	}
	if output.Len() != 0 {
		t.Fatal("panic was reported as a completed decode or leaked panic contents")
	}
}

type browserModelCodecCancelWriter struct{ cancel context.CancelFunc }

func (w browserModelCodecCancelWriter) Write(p []byte) (int, error) {
	if bytes.Contains(p, []byte("warmup_cycle ")) {
		w.cancel()
	}
	return len(p), nil
}

func TestBrowserHTTPModelCodecWarmupAdmission(t *testing.T) {
	path, data := browserHTTPModelFixture(t)
	codec, err := warmBrowserHTTPModelCodec(context.Background(), path)
	if err != nil || codec == nil || codec.prototype.GetStoreId() != "" {
		t.Fatalf("successful full warmup did not return an unbound private codec: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if codec, err = warmBrowserHTTPModelCodec(ctx, path); codec != nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled phase exposed a codec: codec=%v err=%v", codec != nil, err)
	}
	data[len(data)-1] = ' '
	tampered := filepath.Join(t.TempDir(), "model.json")
	if err := os.WriteFile(tampered, data, 0600); err != nil {
		t.Fatal(err)
	}
	if codec, err = warmBrowserHTTPModelCodec(context.Background(), tampered); codec != nil || err == nil {
		t.Fatal("unverified model exposed a codec")
	}
	ctx, cancel = context.WithCancel(context.Background())
	defer cancel()
	previous := browserTransportLogger.Writer()
	browserTransportLogger.SetOutput(browserModelCodecCancelWriter{cancel})
	t.Cleanup(func() { browserTransportLogger.SetOutput(previous) })
	if codec, err = warmBrowserHTTPModelCodec(ctx, path); codec != nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("cancellation before returning a completed phase lost: codec=%v err=%v", codec != nil, err)
	}
}

func TestBrowserHTTPModelCodecConcurrentCopies(t *testing.T) {
	data, codec := browserModelCodecFixture(t)
	previous := browserTransportLogger.Writer()
	browserTransportLogger.SetOutput(io.Discard)
	t.Cleanup(func() { browserTransportLogger.SetOutput(previous) })
	start := make(chan struct{})
	failures := make(chan error, 16)
	var group sync.WaitGroup
	for worker := 0; worker < 16; worker++ {
		group.Add(1)
		go func(worker int) {
			defer group.Done()
			<-start
			for attempt := 0; attempt < 4; attempt++ {
				got := &openfgav1.WriteAuthorizationModelRequest{}
				if err := codec.NewDecoder(bytes.NewReader(data)).Decode(got); err != nil || !proto.Equal(got, codec.prototype) {
					failures <- fmt.Errorf("worker %d decode parity: %v", worker, err)
					return
				}
				got.StoreId = fmt.Sprint(worker)
				got.TypeDefinitions[0].Metadata.Module = fmt.Sprint(worker, attempt)
			}
		}(worker)
	}
	close(start)
	group.Wait()
	close(failures)
	for err := range failures {
		t.Error(err)
	}
}

func BenchmarkBrowserHTTPModelCodec(b *testing.B) {
	data, codec := browserModelCodecFixture(b)
	previous := browserTransportLogger.Writer()
	browserTransportLogger.SetOutput(io.Discard)
	b.Cleanup(func() { browserTransportLogger.SetOutput(previous) })
	for _, bc := range []struct {
		name      string
		marshaler runtime.Marshaler
	}{
		{"default_JSONPb", codec.JSONPb},
		{"pinned_JSON_private_copy", codec},
	} {
		b.Run(bc.name, func(b *testing.B) {
			b.ReportAllocs()
			b.SetBytes(int64(len(data)))
			for i := 0; i < b.N; i++ {
				request := &openfgav1.WriteAuthorizationModelRequest{}
				if err := bc.marshaler.NewDecoder(bytes.NewReader(data)).Decode(request); err != nil {
					b.Fatal(err)
				}
			}
		})
	}
	b.Run("prototype_clone_only", func(b *testing.B) {
		b.ReportAllocs()
		for i := 0; i < b.N; i++ {
			_ = proto.Clone(codec.prototype)
		}
	})
}
