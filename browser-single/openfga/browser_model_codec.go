package run

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"sync/atomic"
	"time"

	"github.com/grpc-ecosystem/grpc-gateway/v2/runtime"
	"github.com/oklog/ulid/v2"
	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// browserModelCodec retains private copies of the pinned request decode and one
// successfully encoded, genuine model-read response. A hit never replaces an
// RPC, datastore read, validation, or authorization decision.
type browserModelCodec struct {
	*runtime.JSONPb
	raw          []byte
	prototype    *openfgav1.WriteAuthorizationModelRequest
	readEncoding atomic.Pointer[browserModelEncoding]
}

// One retained response bounds memory independently of the number of models.
// An oversized response keeps ordinary JSONPb behavior without being retained.
const browserModelEncodingMaxBytes = 1 << 20

type browserModelEncoding struct {
	response *openfgav1.ReadAuthorizationModelResponse
	data     []byte
}

// Preparation does not install the codec. The startup caller may attach it to
// its gateway only after the entire bounded warmup has succeeded.
func newBrowserModelCodec(data []byte) (*browserModelCodec, error) {
	if len(data) != browserHTTPModelBytes || fmt.Sprintf("%x", sha256.Sum256(data)) != browserHTTPModelSHA256 {
		return nil, fmt.Errorf("browser model codec requires the exact pinned JSON")
	}
	jsonCodec := &runtime.JSONPb{
		MarshalOptions:   protojson.MarshalOptions{EmitUnpopulated: true},
		UnmarshalOptions: protojson.UnmarshalOptions{DiscardUnknown: true},
	}
	prototype := &openfgav1.WriteAuthorizationModelRequest{}
	if err := jsonCodec.NewDecoder(bytes.NewReader(data)).Decode(prototype); err != nil {
		return nil, err
	}
	if prototype.GetStoreId() != "" {
		return nil, fmt.Errorf("pinned browser model JSON must not supply a store ID")
	}
	return &browserModelCodec{
		JSONPb:    jsonCodec,
		raw:       bytes.Clone(bytes.TrimSpace(data)),
		prototype: prototype,
	}, nil
}

func (c *browserModelCodec) NewDecoder(r io.Reader) runtime.Decoder {
	options := c.UnmarshalOptions
	// Only the exact options used during preparation may use the cached decode.
	// All other options retain JSONPb's implementation, including error behavior.
	if c.prototype == nil || options.AllowPartial || !options.DiscardUnknown ||
		options.Resolver != nil || options.RecursionLimit != 0 {
		return c.JSONPb.NewDecoder(r)
	}
	return &browserModelDecoder{
		DecoderWrapper: runtime.DecoderWrapper{
			Decoder:          json.NewDecoder(r),
			UnmarshalOptions: options,
		},
		codec: c,
	}
}

type browserModelDecoder struct {
	runtime.DecoderWrapper
	codec *browserModelCodec
}

func (d *browserModelDecoder) Decode(v interface{}) (err error) {
	request, ok := v.(*openfgav1.WriteAuthorizationModelRequest)
	if !ok || request == nil {
		return d.DecoderWrapper.Decode(v)
	}
	started := time.Now()
	cacheHit, completed, size := false, false, 0
	defer func() {
		if !completed {
			return // Preserve panic propagation without reporting a completion.
		}
		browserTransportLogger.Printf("model_decode cache_hit=%t bytes=%d duration_ms=%d failed=%t",
			cacheHit, size, time.Since(started).Milliseconds(), err != nil)
	}()

	// This is JSONPb's original streaming first step. In particular, do not
	// replace it with ReadAll: repeated Decode, reader errors and trailing input
	// must keep their original semantics, including the decoder's read-ahead.
	var raw json.RawMessage
	if err := d.Decoder.Decode(&raw); err != nil {
		completed = true
		return err
	}
	size = len(raw)
	if bytes.Equal(raw, d.codec.raw) {
		proto.Reset(request)
		proto.Merge(request, d.codec.prototype)
		cacheHit = true
		completed = true
		return nil
	}
	err = d.UnmarshalOptions.Unmarshal(raw, request)
	completed = true
	return err
}

// Marshal is the unary gateway's response path. The generated handler must
// finish the real RPC successfully before calling it. NewEncoder deliberately
// remains JSONPb's original implementation, so startup fixtures cannot seed this
// entry. All IDs and unknown fields participate in an entire-response hit.
func (c *browserModelCodec) Marshal(v interface{}) (out []byte, err error) {
	response, ok := v.(*openfgav1.ReadAuthorizationModelResponse)
	if !ok || response == nil {
		return c.JSONPb.Marshal(v)
	}
	started := time.Now()
	cacheHit, cacheStored, completed := false, false, false
	defer func() {
		if completed {
			browserTransportLogger.Printf("model_encode cache_hit=%t cache_stored=%t bytes=%d duration_ms=%d failed=%t",
				cacheHit, cacheStored, len(out), time.Since(started).Milliseconds(), err != nil)
		}
	}()

	options := c.MarshalOptions
	// MarshalOptions is exported by JSONPb. A per-instance entry alone does not
	// protect callers that select different output options after its creation.
	supported := !options.Multiline && options.Indent == "" && !options.AllowPartial &&
		!options.UseProtoNames && !options.UseEnumNumbers && options.EmitUnpopulated &&
		!options.EmitDefaultValues && options.Resolver == nil
	retained := c.readEncoding.Load()
	if supported && retained != nil && proto.Equal(retained.response, response) {
		out = bytes.Clone(retained.data)
		cacheHit, completed = true, true
		return out, nil
	}
	eligible := supported && retained == nil && c.matchesReadModel(response)
	out, err = c.JSONPb.Marshal(v)
	if err == nil && eligible && len(out) <= browserModelEncodingMaxBytes {
		// Neither returned bytes nor the real RPC's message can mutate this key.
		prepared := &browserModelEncoding{
			response: proto.Clone(response).(*openfgav1.ReadAuthorizationModelResponse),
			data:     bytes.Clone(out),
		}
		cacheStored = c.readEncoding.CompareAndSwap(nil, prepared)
	}
	completed = true
	return out, err
}

func (c *browserModelCodec) matchesReadModel(response *openfgav1.ReadAuthorizationModelResponse) bool {
	model := response.GetAuthorizationModel()
	if c.prototype == nil || model == nil || model.GetId() == "" ||
		model.GetId() == browserHTTPWarmupID || len(model.GetConditions()) != 0 ||
		len(response.ProtoReflect().GetUnknown()) != 0 {
		return false
	}
	if _, err := ulid.ParseStrict(model.GetId()); err != nil {
		return false
	}
	// Compare all policy fields without copying MessageState, changing the
	// incoming ID, or aliasing the later retained key. Descriptor tests guard
	// this complete view against a future generated-message schema change.
	want := &openfgav1.AuthorizationModel{
		SchemaVersion:   c.prototype.GetSchemaVersion(),
		TypeDefinitions: c.prototype.GetTypeDefinitions(),
		Conditions:      c.prototype.GetConditions(),
	}
	body := &openfgav1.AuthorizationModel{
		SchemaVersion:   model.GetSchemaVersion(),
		TypeDefinitions: model.GetTypeDefinitions(),
		Conditions:      model.GetConditions(),
	}
	body.ProtoReflect().SetUnknown(model.ProtoReflect().GetUnknown())
	return proto.Equal(want, body)
}
