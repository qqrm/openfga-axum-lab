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
	browserHTTPWarmupCycles  = 1
	browserHTTPModelBytes    = 52709
	browserHTTPModelSHA256   = "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
	// Only codec input: this ID is never written to a store or returned by an RPC.
	browserHTTPWarmupID = "01J00000000000000000000000"
)

const (
	browserWarmupJSONDecode = iota
	browserWarmupProtobuf
	browserWarmupGeneratedValidation
	browserWarmupTypeSystem
	browserWarmupResponseCodec
	browserWarmupStageCount
)

type browserHTTPWarmupProgress struct {
	cycle        int
	stage        int
	stageStarted time.Time
	durations    [browserWarmupStageCount]time.Duration
	completed    bool
	failed       bool
}

// Run before datastore construction or any listener. Exercise the real codecs
// and fully validate the pinned model once. Publish only the immutable fact that
// this exact model body is valid after every startup stage succeeds; never cache
// a TypeSystem, datastore state or an authorization decision.
func warmBrowserHTTPModel(ctx context.Context, path string) error {
	started := time.Now()
	ctx, cancel := context.WithTimeout(ctx, browserHTTPWarmupTimeout)
	defer cancel()
	var prepared *typesystem.BrowserModelValidation
	// Five stage starts and one completion per cycle fit even after the caller
	// times out. The worker sends value snapshots and never logs after timeout.
	progress := make(chan browserHTTPWarmupProgress, browserHTTPWarmupCycles*(browserWarmupStageCount+1))
	browserTransportLogger.Printf("warmup_start model_sha256=%s model_bytes=%d cycles=%d timeout_seconds=%d", browserHTTPModelSHA256, browserHTTPModelBytes, browserHTTPWarmupCycles, int(browserHTTPWarmupTimeout/time.Second))
	err := runBrowserHTTPWarmup(ctx, browserHTTPWarmupTimeout, func(ctx context.Context) error {
		// Reading, identity verification, codecs and validation share this context.
		data, err := readBrowserHTTPModel(path)
		if err != nil {
			return err
		}
		prepared, err = warmBrowserHTTPModelCycles(ctx, data, progress)
		return err
	})
	if err == nil {
		// The result receive synchronizes access to prepared. On timeout the
		// caller never reads the worker result and never installs its cache.
		err = typesystem.InstallBrowserModelValidation(ctx, prepared)
	}
	logBrowserHTTPWarmupCycles(progress)
	browserTransportLogger.Printf("warmup_end duration_ms=%d failed=%t", time.Since(started).Milliseconds(), err != nil)
	return err
}

func warmBrowserHTTPModelCycles(ctx context.Context, data []byte, progress chan<- browserHTTPWarmupProgress) (*typesystem.BrowserModelValidation, error) {
	var prepared *typesystem.BrowserModelValidation
	for cycle := 1; cycle <= browserHTTPWarmupCycles; cycle++ {
		var err error
		prepared, err = warmBrowserHTTPModelCodecs(ctx, data, cycle, progress)
		if err != nil {
			return nil, err
		}
	}
	return prepared, ctx.Err()
}

// Log one latest snapshot per started cycle. On timeout this includes the
// partial duration of synchronous work still running; it is not a completion.
func logBrowserHTTPWarmupCycles(progress <-chan browserHTTPWarmupProgress) {
	var latest [browserHTTPWarmupCycles]browserHTTPWarmupProgress
drain:
	for {
		select {
		case update := <-progress:
			latest[update.cycle-1] = update
		default:
			break drain
		}
	}
	stages := [...]string{"json_decode", "protobuf", "generated_validation", "typesystem", "response_codec"}
	for _, cycle := range latest {
		if cycle.cycle == 0 {
			continue
		}
		stage := stages[cycle.stage]
		if cycle.completed {
			stage = "complete"
		} else if !cycle.failed {
			cycle.durations[cycle.stage] = time.Since(cycle.stageStarted)
			cycle.failed = true
		}
		browserTransportLogger.Printf("warmup_cycle cycle=%d completed=%t current_stage=%s failed=%t json_decode_ms=%d protobuf_ms=%d generated_validation_ms=%d typesystem_ms=%d response_codec_ms=%d",
			cycle.cycle, cycle.completed, stage, cycle.failed,
			cycle.durations[browserWarmupJSONDecode].Milliseconds(), cycle.durations[browserWarmupProtobuf].Milliseconds(),
			cycle.durations[browserWarmupGeneratedValidation].Milliseconds(), cycle.durations[browserWarmupTypeSystem].Milliseconds(),
			cycle.durations[browserWarmupResponseCodec].Milliseconds())
	}
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

func warmBrowserHTTPModelCodecs(ctx context.Context, data []byte, cycle int, progress chan<- browserHTTPWarmupProgress) (prepared *typesystem.BrowserModelValidation, err error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	current := browserHTTPWarmupProgress{cycle: cycle}
	startStage := func(stage int) {
		now := time.Now()
		if !current.stageStarted.IsZero() {
			current.durations[current.stage] = now.Sub(current.stageStarted)
		}
		current.stage, current.stageStarted = stage, now
		progress <- current
	}
	startStage(browserWarmupJSONDecode)
	defer func() {
		current.durations[current.stage] = time.Since(current.stageStarted)
		current.completed, current.failed = err == nil, err != nil
		progress <- current
	}()
	// These are grpc-gateway's pinned default JSONPb options. Use the full model,
	// so nested messages/maps/oneofs take the same codec paths as the real POST.
	codec := &runtime.JSONPb{
		MarshalOptions:   protojson.MarshalOptions{EmitUnpopulated: true},
		UnmarshalOptions: protojson.UnmarshalOptions{DiscardUnknown: true},
	}
	request := &openfgav1.WriteAuthorizationModelRequest{}
	if err := codec.NewDecoder(bytes.NewReader(data)).Decode(request); err != nil {
		return nil, fmt.Errorf("warmup gateway model decode: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	startStage(browserWarmupProtobuf)
	request.StoreId = browserHTTPWarmupID
	wire, err := proto.Marshal(request)
	if err != nil {
		return nil, fmt.Errorf("warmup request protobuf encode: %w", err)
	}
	decoded := &openfgav1.WriteAuthorizationModelRequest{}
	if err := proto.Unmarshal(wire, decoded); err != nil {
		return nil, fmt.Errorf("warmup request protobuf decode: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	startStage(browserWarmupGeneratedValidation)
	if err := decoded.Validate(); err != nil {
		return nil, fmt.Errorf("warmup generated request validation: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	startStage(browserWarmupTypeSystem)
	model := &openfgav1.AuthorizationModel{
		Id:              browserHTTPWarmupID,
		SchemaVersion:   decoded.GetSchemaVersion(),
		TypeDefinitions: decoded.GetTypeDefinitions(),
		Conditions:      decoded.GetConditions(),
	}
	prepared, err = typesystem.PrepareBrowserModelValidation(ctx, model)
	if err != nil {
		return nil, fmt.Errorf("warmup type-system validation: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	startStage(browserWarmupResponseCodec)
	// Serialization fixtures only: no service handler, datastore or Check call.
	// Encoding the full read response also touches the model's JSON encoders.
	for _, response := range []proto.Message{
		&openfgav1.WriteAuthorizationModelResponse{AuthorizationModelId: browserHTTPWarmupID},
		&openfgav1.ReadAuthorizationModelResponse{AuthorizationModel: model},
	} {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		wire, err := proto.Marshal(response)
		if err != nil {
			return nil, fmt.Errorf("warmup response protobuf encode: %w", err)
		}
		decodedResponse := response.ProtoReflect().New().Interface()
		if err := proto.Unmarshal(wire, decodedResponse); err != nil {
			return nil, fmt.Errorf("warmup response protobuf decode: %w", err)
		}
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		if err := codec.NewEncoder(io.Discard).Encode(decodedResponse); err != nil {
			return nil, fmt.Errorf("warmup gateway response encode: %w", err)
		}
	}
	return prepared, ctx.Err()
}
