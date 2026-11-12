package run

import (
	"bytes"
	"context"
	"crypto/sha256"
	"fmt"
	"io"
	"os"
	"time"

	"github.com/grpc-ecosystem/grpc-gateway/v2/runtime"
	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	"github.com/openfga/openfga/pkg/typesystem"
)

const (
	browserHTTPWarmupTimeout = 60 * time.Second
	browserHTTPModelBytes    = 52709
	browserHTTPModelSHA256   = "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
	// Only codec input: this ID is never written to a store or returned by an RPC.
	browserHTTPWarmupID = "01J00000000000000000000000"
)

// Run before datastore construction or any listener. All decoded messages and
// the TypeSystem are local and discarded. Real requests still decode, validate,
// build a fresh TypeSystem and execute the unchanged datastore operations.
func warmBrowserHTTPModel(ctx context.Context, path string) error {
	started := time.Now()
	browserTransportLogger.Printf("warmup_start model_sha256=%s model_bytes=%d timeout_seconds=%d", browserHTTPModelSHA256, browserHTTPModelBytes, int(browserHTTPWarmupTimeout/time.Second))
	err := runBrowserHTTPWarmup(ctx, browserHTTPWarmupTimeout, func(ctx context.Context) error {
		data, err := readBrowserHTTPModel(path)
		if err != nil {
			return err
		}
		return warmBrowserHTTPModelCodecs(ctx, data)
	})
	browserTransportLogger.Printf("warmup_end duration_ms=%d failed=%t", time.Since(started).Milliseconds(), err != nil)
	return err
}

// Synchronous codec/validation calls cannot themselves be interrupted. Bound
// the startup caller's wait, check cancellation between stages, and never let
// the worker publish readiness. Its buffered result cannot block after timeout.
// The CLI exits on the returned startup error; no listener has opened yet.
func runBrowserHTTPWarmup(ctx context.Context, budget time.Duration, work func(context.Context) error) error {
	ctx, cancel := context.WithTimeout(ctx, budget)
	defer cancel()
	if err := ctx.Err(); err != nil {
		return err
	}
	result := make(chan error, 1)
	go func() { result <- work(ctx) }()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case err := <-result:
		// Cancellation must win even when completion and the deadline race.
		if contextError := ctx.Err(); contextError != nil {
			return contextError
		}
		return err
	}
}

func readBrowserHTTPModel(path string) ([]byte, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("read pinned warmup model: %w", err)
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() {
		return nil, fmt.Errorf("pinned warmup model must be a regular file")
	}
	data, err := io.ReadAll(io.LimitReader(file, browserHTTPModelBytes+1))
	if err != nil {
		return nil, err
	}
	if len(data) != browserHTTPModelBytes {
		return nil, fmt.Errorf("pinned warmup model size: got %d, want %d bytes", len(data), browserHTTPModelBytes)
	}
	if digest := fmt.Sprintf("%x", sha256.Sum256(data)); digest != browserHTTPModelSHA256 {
		return nil, fmt.Errorf("pinned warmup model checksum mismatch: got %s", digest)
	}
	return data, nil
}

func warmBrowserHTTPModelCodecs(ctx context.Context, data []byte) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	// These are grpc-gateway's pinned default JSONPb options. Use the full model,
	// so nested messages/maps/oneofs take the same codec paths as the real POST.
	codec := &runtime.JSONPb{
		MarshalOptions:   protojson.MarshalOptions{EmitUnpopulated: true},
		UnmarshalOptions: protojson.UnmarshalOptions{DiscardUnknown: true},
	}
	request := &openfgav1.WriteAuthorizationModelRequest{}
	if err := codec.NewDecoder(bytes.NewReader(data)).Decode(request); err != nil {
		return fmt.Errorf("warmup gateway model decode: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	request.StoreId = browserHTTPWarmupID
	wire, err := proto.Marshal(request)
	if err != nil {
		return fmt.Errorf("warmup request protobuf encode: %w", err)
	}
	decoded := &openfgav1.WriteAuthorizationModelRequest{}
	if err := proto.Unmarshal(wire, decoded); err != nil {
		return fmt.Errorf("warmup request protobuf decode: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := decoded.Validate(); err != nil {
		return fmt.Errorf("warmup generated request validation: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	model := &openfgav1.AuthorizationModel{
		Id:              browserHTTPWarmupID,
		SchemaVersion:   decoded.GetSchemaVersion(),
		TypeDefinitions: decoded.GetTypeDefinitions(),
		Conditions:      decoded.GetConditions(),
	}
	if _, err := typesystem.NewAndValidate(ctx, model); err != nil {
		return fmt.Errorf("warmup type-system validation: %w", err)
	}
	// Serialization fixtures only: no service handler, datastore or Check call.
	// Encoding the full read response also touches the model's JSON encoders.
	for _, response := range []proto.Message{
		&openfgav1.WriteAuthorizationModelResponse{AuthorizationModelId: browserHTTPWarmupID},
		&openfgav1.ReadAuthorizationModelResponse{AuthorizationModel: model},
	} {
		if err := ctx.Err(); err != nil {
			return err
		}
		wire, err := proto.Marshal(response)
		if err != nil {
			return fmt.Errorf("warmup response protobuf encode: %w", err)
		}
		decodedResponse := response.ProtoReflect().New().Interface()
		if err := proto.Unmarshal(wire, decodedResponse); err != nil {
			return fmt.Errorf("warmup response protobuf decode: %w", err)
		}
		if err := ctx.Err(); err != nil {
			return err
		}
		if err := codec.NewEncoder(io.Discard).Encode(decodedResponse); err != nil {
			return fmt.Errorf("warmup gateway response encode: %w", err)
		}
	}
	return ctx.Err()
}
