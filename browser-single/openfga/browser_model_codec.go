package run

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"time"

	"github.com/grpc-ecosystem/grpc-gateway/v2/runtime"
	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// browserModelCodec retains only the result of decoding the exact pinned JSON.
// Each hit copies into an independent request. It never supplies a store ID,
// model ID, validation flag, datastore result, or authorization decision.
type browserModelCodec struct {
	*runtime.JSONPb
	raw       []byte
	prototype *openfgav1.WriteAuthorizationModelRequest
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
