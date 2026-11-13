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
	"reflect"
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

func browserModelCodecReadFixture(codec *browserModelCodec, id string) *openfgav1.ReadAuthorizationModelResponse {
	request := proto.Clone(codec.prototype).(*openfgav1.WriteAuthorizationModelRequest)
	return &openfgav1.ReadAuthorizationModelResponse{AuthorizationModel: &openfgav1.AuthorizationModel{
		Id:              id,
		SchemaVersion:   request.GetSchemaVersion(),
		TypeDefinitions: request.GetTypeDefinitions(),
		Conditions:      request.GetConditions(),
	}}
}

func assertBrowserModelEncodingParity(t *testing.T, codec *browserModelCodec, value interface{}) []byte {
	t.Helper()
	want, wantErr := codec.JSONPb.Marshal(value)
	got, gotErr := codec.Marshal(value)
	if fmt.Sprintf("%T:%v", gotErr, gotErr) != fmt.Sprintf("%T:%v", wantErr, wantErr) {
		t.Fatalf("encoding error parity: got %T:%v; want %T:%v", gotErr, gotErr, wantErr, wantErr)
	}
	if gotErr != nil {
		return got
	}
	// protojson does not promise stable insignificant whitespace across builds.
	var gotJSON, wantJSON interface{}
	if err := json.Unmarshal(got, &gotJSON); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(want, &wantJSON); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(gotJSON, wantJSON) {
		t.Fatal("candidate changed the actual response JSON")
	}
	return got
}

func TestBrowserHTTPModelCodecReadEncodingCopies(t *testing.T) {
	_, codec := browserModelCodecFixture(t)
	output := captureBrowserHTTPWarmupLog(t)
	response := browserModelCodecReadFixture(codec, "01J00000000000000000000001")
	original := proto.Clone(response).(*openfgav1.ReadAuthorizationModelResponse)
	first := assertBrowserModelEncodingParity(t, codec, response)
	retained := codec.readEncoding.Load()
	if retained == nil || !proto.Equal(retained.response, original) ||
		len(retained.data) > browserModelEncodingMaxBytes ||
		!strings.Contains(output.String(), "cache_stored=true") {
		t.Fatal("successful eligible response did not create the bounded private entry")
	}
	response.AuthorizationModel.Id = "01J00000000000000000000002"
	response.AuthorizationModel.TypeDefinitions[0].Metadata.Module = "changed"
	response.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01})
	for i := range first {
		first[i] = 'x'
	}
	output.Reset()
	second := assertBrowserModelEncodingParity(t, codec, original)
	if !strings.Contains(output.String(), "model_encode cache_hit=true") {
		t.Fatal("identical complete response did not reuse its encoding")
	}
	for i := range second {
		second[i] = 'y'
	}
	_ = assertBrowserModelEncodingParity(t, codec, original)
	if codec.readEncoding.Load() != retained || !proto.Equal(retained.response, original) ||
		!json.Valid(retained.data) || strings.Contains(output.String(), original.GetAuthorizationModel().GetId()) ||
		strings.Contains(output.String(), "schema_version") {
		t.Fatal("caller mutated the private key/bytes, or diagnostics leaked response content")
	}
}

func TestBrowserHTTPModelCodecReadEncodingFallbacks(t *testing.T) {
	_, codec := browserModelCodecFixture(t)
	output := captureBrowserHTTPWarmupLog(t)
	original := browserModelCodecReadFixture(codec, "01J00000000000000000000001")
	_ = assertBrowserModelEncodingParity(t, codec, original)
	retained := codec.readEncoding.Load()
	cases := []struct {
		name   string
		change func(*openfgav1.ReadAuthorizationModelResponse)
	}{
		{"other_id", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.Id = "01J00000000000000000000002" }},
		{"empty_id", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.Id = "" }},
		{"invalid_id", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.Id = "invalid" }},
		{"warmup_id", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.Id = browserHTTPWarmupID }},
		{"schema", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.SchemaVersion = "invalid" }},
		{"metadata", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.TypeDefinitions[0].Metadata.Module = "changed" }},
		{"conditions", func(r *openfgav1.ReadAuthorizationModelResponse) {
			r.AuthorizationModel.Conditions = map[string]*openfgav1.Condition{"extra": {Name: "extra", Expression: "true"}}
		}},
		{"response_unknown", func(r *openfgav1.ReadAuthorizationModelResponse) { r.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01}) }},
		{"model_unknown", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01}) }},
		{"nested_unknown", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.TypeDefinitions[0].ProtoReflect().SetUnknown([]byte{0x98, 0x06, 0x01}) }},
		{"missing_model", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel = nil }},
		{"invalid_utf8", func(r *openfgav1.ReadAuthorizationModelResponse) { r.AuthorizationModel.TypeDefinitions[0].Type = string([]byte{0xff}) }},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			response := proto.Clone(original).(*openfgav1.ReadAuthorizationModelResponse)
			tc.change(response)
			output.Reset()
			_ = assertBrowserModelEncodingParity(t, codec, response)
			if strings.Contains(output.String(), "cache_hit=true") || codec.readEncoding.Load() != retained {
				t.Fatal("mismatch reused or replaced the first response")
			}
			if tc.name != "other_id" {
				_, fresh := browserModelCodecFixture(t)
				_ = assertBrowserModelEncodingParity(t, fresh, response)
				if fresh.readEncoding.Load() != nil {
					t.Fatal("unsupported response was admitted into an empty entry")
				}
			}
		})
	}
	for _, response := range []interface{}{
		nil, (*openfgav1.ReadAuthorizationModelResponse)(nil),
		&openfgav1.WriteAuthorizationModelResponse{AuthorizationModelId: "01J00000000000000000000001"},
		map[string]string{"kind": "unrelated"},
	} {
		_ = assertBrowserModelEncodingParity(t, codec, response)
	}
	if codec.readEncoding.Load() != retained {
		t.Fatal("another output type replaced the retained response")
	}
}

func TestBrowserHTTPModelCodecReadEncodingOptions(t *testing.T) {
	for _, tc := range []struct {
		name   string
		change func(*protojson.MarshalOptions)
	}{
		{"multiline", func(o *protojson.MarshalOptions) { o.Multiline = true }},
		{"indent", func(o *protojson.MarshalOptions) { o.Indent = "  " }},
		{"allow_partial", func(o *protojson.MarshalOptions) { o.AllowPartial = true }},
		{"proto_names", func(o *protojson.MarshalOptions) { o.UseProtoNames = true }},
		{"enum_numbers", func(o *protojson.MarshalOptions) { o.UseEnumNumbers = true }},
		{"omit_unpopulated", func(o *protojson.MarshalOptions) { o.EmitUnpopulated = false }},
		{"emit_defaults", func(o *protojson.MarshalOptions) { o.EmitDefaultValues = true }},
		{"resolver", func(o *protojson.MarshalOptions) { o.Resolver = protoregistry.GlobalTypes }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, codec := browserModelCodecFixture(t)
			output := captureBrowserHTTPWarmupLog(t)
			response := browserModelCodecReadFixture(codec, "01J00000000000000000000001")
			_ = assertBrowserModelEncodingParity(t, codec, response)
			retained := codec.readEncoding.Load()
			tc.change(&codec.MarshalOptions)
			output.Reset()
			_ = assertBrowserModelEncodingParity(t, codec, response)
			if strings.Contains(output.String(), "cache_hit=true") || codec.readEncoding.Load() != retained {
				t.Fatal("changed marshal options reused or replaced the old encoding")
			}
			_, fresh := browserModelCodecFixture(t)
			fresh.MarshalOptions = codec.MarshalOptions
			_ = assertBrowserModelEncodingParity(t, fresh, response)
			if fresh.readEncoding.Load() != nil {
				t.Fatal("non-default options published a retained encoding")
			}
			codec.MarshalOptions = protojson.MarshalOptions{EmitUnpopulated: true}
			output.Reset()
			_ = assertBrowserModelEncodingParity(t, codec, response)
			if !strings.Contains(output.String(), "cache_hit=true") {
				t.Fatal("unchanged default encoding was lost after an ordinary fallback")
			}
		})
	}
}

func TestBrowserHTTPModelCodecReadEncodingAdmissionAndSchema(t *testing.T) {
	path, _ := browserHTTPModelFixture(t)
	codec, err := warmBrowserHTTPModelCodec(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	if codec.readEncoding.Load() != nil {
		t.Fatal("startup NewEncoder fixture seeded a real response entry")
	}
	response := browserModelCodecReadFixture(codec, "01J00000000000000000000001")
	if err := codec.NewEncoder(io.Discard).Encode(response); err != nil || codec.readEncoding.Load() != nil {
		t.Fatal("ordinary NewEncoder changed its behavior or seeded the unary cache")
	}
	for _, tc := range []struct {
		message proto.Message
		names   []string
	}{
		{&openfgav1.AuthorizationModel{}, []string{"id", "type_definitions", "schema_version", "conditions"}},
		{&openfgav1.ReadAuthorizationModelResponse{}, []string{"authorization_model"}},
		{&openfgav1.WriteAuthorizationModelRequest{}, []string{"store_id", "type_definitions", "schema_version", "conditions"}},
	} {
		fields := tc.message.ProtoReflect().Descriptor().Fields()
		if fields.Len() != len(tc.names) {
			t.Fatal("generated model schema changed; review the complete policy comparison")
		}
		want := make(map[string]bool, len(tc.names))
		for _, name := range tc.names {
			want[name] = true
		}
		for i := 0; i < fields.Len(); i++ {
			if !want[string(fields.Get(i).Name())] {
				t.Fatal("generated field is absent from the policy comparison")
			}
		}
	}
	options := reflect.TypeOf(protojson.MarshalOptions{})
	expectedOptions := map[string]bool{
		"Multiline": true, "Indent": true, "AllowPartial": true,
		"UseProtoNames": true, "UseEnumNumbers": true, "EmitUnpopulated": true,
		"EmitDefaultValues": true, "Resolver": true,
	}
	covered := 0
	for i := 0; i < options.NumField(); i++ {
		field := options.Field(i)
		if !field.IsExported() || field.Name == "NoUnkeyedLiterals" {
			continue
		}
		if !expectedOptions[field.Name] {
			t.Fatalf("new MarshalOptions field %s needs an explicit cache guard", field.Name)
		}
		covered++
	}
	if covered != len(expectedOptions) {
		t.Fatal("MarshalOptions shape changed; review the exact output-options guard")
	}
	// This deliberately enlarged private test fixture exercises only retention's
	// hard bound. It is not a gateway-accepted model or a changed production pin.
	codec.prototype.TypeDefinitions[0].Metadata.Module = strings.Repeat("x", browserModelEncodingMaxBytes+1)
	large := browserModelCodecReadFixture(codec, "01J00000000000000000000001")
	out := assertBrowserModelEncodingParity(t, codec, large)
	if len(out) <= browserModelEncodingMaxBytes || codec.readEncoding.Load() != nil {
		t.Fatal("oversized serialized response was retained or not tested")
	}
}

func TestBrowserHTTPModelCodecReadEncodingConcurrentCopies(t *testing.T) {
	_, codec := browserModelCodecFixture(t)
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
			id := fmt.Sprintf("01J0000000000000000000000%d", 1+worker%2)
			for attempt := 0; attempt < 4; attempt++ {
				response := browserModelCodecReadFixture(codec, id)
				out, err := codec.Marshal(response)
				got := &openfgav1.ReadAuthorizationModelResponse{}
				if err != nil || protojson.Unmarshal(out, got) != nil || !proto.Equal(got, response) {
					failures <- fmt.Errorf("worker %d response parity: %v", worker, err)
					return
				}
				for i := range out {
					out[i] = 'x'
				}
				response.AuthorizationModel.TypeDefinitions[0].Metadata.Module = fmt.Sprint(worker, attempt)
			}
		}(worker)
	}
	close(start)
	group.Wait()
	close(failures)
	for err := range failures {
		t.Error(err)
	}
	retained := codec.readEncoding.Load()
	if retained == nil || len(retained.data) > browserModelEncodingMaxBytes {
		t.Fatal("concurrent eligible calls did not leave one bounded entry")
	}
	id := retained.response.GetAuthorizationModel().GetId()
	if id != "01J00000000000000000000001" && id != "01J00000000000000000000002" {
		t.Fatal("entry lost the real winning response ID")
	}
	original := browserModelCodecReadFixture(codec, id)
	if !proto.Equal(retained.response, original) || !json.Valid(retained.data) {
		t.Fatal("concurrent callers changed the retained key or bytes")
	}
	_ = assertBrowserModelEncodingParity(t, codec, browserModelCodecReadFixture(codec, "01J00000000000000000000003"))
	if codec.readEncoding.Load() != retained {
		t.Fatal("a later valid model replaced the first retained response")
	}
}
