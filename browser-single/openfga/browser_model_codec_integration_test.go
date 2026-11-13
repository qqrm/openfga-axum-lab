package run

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	grpcauth "github.com/grpc-ecosystem/go-grpc-middleware/v2/interceptors/auth"
	"github.com/grpc-ecosystem/grpc-gateway/v2/runtime"
	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/grpc"
	healthv1pb "google.golang.org/grpc/health/grpc_health_v1"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	"github.com/openfga/openfga/internal/authn"
	"github.com/openfga/openfga/internal/authn/presharedkey"
	authnmw "github.com/openfga/openfga/internal/middleware/authn"
	"github.com/openfga/openfga/pkg/gateway"
	"github.com/openfga/openfga/pkg/logger"
	"github.com/openfga/openfga/pkg/middleware"
	httpmiddleware "github.com/openfga/openfga/pkg/middleware/http"
	"github.com/openfga/openfga/pkg/middleware/validator"
	fgaserver "github.com/openfga/openfga/pkg/server"
	serverconfig "github.com/openfga/openfga/pkg/server/config"
	serverErrors "github.com/openfga/openfga/pkg/server/errors"
	"github.com/openfga/openfga/pkg/server/health"
	"github.com/openfga/openfga/pkg/storage"
	"github.com/openfga/openfga/pkg/storage/memory"
)

const browserCodecTestToken = "local-codec-regression-key"

type browserCodecGateway struct {
	url       string
	client    *http.Client
	datastore storage.OpenFGADatastore
}

// This is a native transport regression, not PostgreSQL/browser acceptance.
// The generated HTTP handler uses an actual TCP gRPC client; all successful
// writes, reads, store IDs, model IDs and health responses come from OpenFGA.
func newBrowserCodecGateway(t *testing.T, codec runtime.Marshaler, requireAuth bool, extra grpc.UnaryServerInterceptor, observe func(*http.Request, []byte)) *browserCodecGateway {
	t.Helper()
	ds := memory.New()
	t.Cleanup(ds.Close)
	log := logger.NewNoopLogger()
	service, err := fgaserver.NewServerWithOpts(fgaserver.WithDatastore(ds), fgaserver.WithTransport(gateway.NewRPCTransport(log)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(service.Close)
	var authenticator authn.Authenticator = authn.NoopAuthenticator{}
	if requireAuth {
		authenticator, err = presharedkey.NewPresharedKeyAuthenticator([]string{browserCodecTestToken})
		if err != nil {
			t.Fatal(err)
		}
	}
	interceptors := []grpc.UnaryServerInterceptor{
		middleware.NewTimeoutInterceptor(serverconfig.DefaultConfig().RequestTimeout, log).NewUnaryTimeoutInterceptor(),
		validator.UnaryServerInterceptor(),
		grpcauth.UnaryServerInterceptor(authnmw.AuthFunc(authenticator)),
	}
	if extra != nil {
		interceptors = append(interceptors, extra)
	}
	rpc := grpc.NewServer(grpc.ChainUnaryInterceptor(interceptors...))
	openfgav1.RegisterOpenFGAServiceServer(rpc, service)
	healthv1pb.RegisterHealthServer(rpc, &health.Checker{TargetService: service, TargetServiceName: openfgav1.OpenFGAService_ServiceDesc.ServiceName})
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	go func() { defer close(done); _ = rpc.Serve(listener) }()
	t.Cleanup(func() { rpc.Stop(); _ = listener.Close(); <-done })
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	// nolint:staticcheck // Match pinned production loopback dial options.
	conn, err := grpc.DialContext(ctx, listener.Addr().String(), browserDialOptions()...)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	mux := runtime.NewServeMux(
		runtime.WithMarshalerOption(runtime.MIMEWildcard, &runtime.HTTPBodyMarshaler{Marshaler: codec}),
		runtime.WithForwardResponseOption(httpmiddleware.HTTPResponseModifier),
		runtime.WithErrorHandler(func(ctx context.Context, _ *runtime.ServeMux, _ runtime.Marshaler, w http.ResponseWriter, r *http.Request, err error) {
			code := serverErrors.ConvertToEncodedErrorCode(status.Convert(err))
			httpmiddleware.CustomHTTPErrorHandler(ctx, w, r, serverErrors.NewEncodedError(code, err.Error()))
		}),
		runtime.WithHealthzEndpoint(healthv1pb.NewHealthClient(conn)),
		runtime.WithOutgoingHeaderMatcher(func(key string) (string, bool) { return key, true }),
	)
	if err := openfgav1.RegisterOpenFGAServiceHandler(context.Background(), mux, conn); err != nil {
		t.Fatal(err)
	}
	var handler http.Handler = mux
	if observe != nil {
		handler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method == http.MethodPost && strings.HasSuffix(r.URL.Path, "/authorization-models") {
				body, err := io.ReadAll(r.Body)
				if err != nil {
					http.Error(w, err.Error(), http.StatusBadRequest)
					return
				}
				observe(r, body)
				r.Body = io.NopCloser(bytes.NewReader(body))
			}
			mux.ServeHTTP(w, r)
		})
	}
	httpServer := httptest.NewServer(handler)
	t.Cleanup(httpServer.Close)
	client := httpServer.Client()
	client.Timeout = 5 * time.Second
	return &browserCodecGateway{url: httpServer.URL, client: client, datastore: ds}
}

func browserCodecDefaultJSON() *runtime.JSONPb {
	return &runtime.JSONPb{MarshalOptions: protojson.MarshalOptions{EmitUnpopulated: true}, UnmarshalOptions: protojson.UnmarshalOptions{DiscardUnknown: true}}
}

func (g *browserCodecGateway) request(t *testing.T, method, path string, body []byte, token string) (int, []byte) {
	t.Helper()
	r, err := http.NewRequest(method, g.url+path, bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	r.Header.Set("Content-Type", "application/json")
	if token != "" {
		r.Header.Set("Authorization", "Bearer "+token)
	}
	response, err := g.client.Do(r)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	data, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return response.StatusCode, data
}

func (g *browserCodecGateway) createStore(t *testing.T) string {
	t.Helper()
	code, data := g.request(t, http.MethodPost, "/stores", []byte(`{"name":"real-codec-regression"}`), browserCodecTestToken)
	var response openfgav1.CreateStoreResponse
	if code != http.StatusCreated || protojson.Unmarshal(data, &response) != nil || response.GetId() == "" {
		t.Fatalf("real CreateStore failed: HTTP %d %s", code, data)
	}
	return response.GetId()
}

func (g *browserCodecGateway) assertModelCount(t *testing.T, storeID string, want int) {
	t.Helper()
	models, continuation, err := g.datastore.ReadAuthorizationModels(context.Background(), storeID, storage.ReadAuthorizationModelsOptions{})
	if err != nil || continuation != "" || len(models) != want {
		t.Fatalf("stored models in %s: got %d, continuation=%q, err=%v, want %d", storeID, len(models), continuation, err, want)
	}
}

func (g *browserCodecGateway) assertWrittenModel(t *testing.T, storeID string, body, response []byte) string {
	t.Helper()
	var created openfgav1.WriteAuthorizationModelResponse
	if err := protojson.Unmarshal(response, &created); err != nil || created.GetAuthorizationModelId() == "" {
		t.Fatalf("invalid real write response: %s (%v)", response, err)
	}
	code, readBody := g.request(t, http.MethodGet, "/stores/"+storeID+"/authorization-models/"+created.GetAuthorizationModelId(), nil, browserCodecTestToken)
	var read openfgav1.ReadAuthorizationModelResponse
	if code != http.StatusOK || protojson.Unmarshal(readBody, &read) != nil {
		t.Fatalf("real read failed: HTTP %d %s", code, readBody)
	}
	stored, err := g.datastore.ReadAuthorizationModel(context.Background(), storeID, created.GetAuthorizationModelId())
	if err != nil || !proto.Equal(stored, read.GetAuthorizationModel()) {
		t.Fatalf("HTTP read differs from real datastore: %v", err)
	}
	var expected openfgav1.WriteAuthorizationModelRequest
	if err := browserCodecDefaultJSON().NewDecoder(bytes.NewReader(body)).Decode(&expected); err != nil {
		t.Fatal(err)
	}
	want := &openfgav1.AuthorizationModel{Id: created.GetAuthorizationModelId(), SchemaVersion: expected.GetSchemaVersion(), TypeDefinitions: expected.GetTypeDefinitions(), Conditions: expected.GetConditions()}
	if !proto.Equal(want, stored) || stored.GetId() == browserHTTPWarmupID {
		t.Fatal("real stored model differs from ordinary decoded request or reuses warmup ID")
	}
	return stored.GetId()
}

func TestBrowserHTTPModelCodecGatewayWriteReadAndFailures(t *testing.T) {
	path, fixture := browserHTTPModelFixture(t)
	codec, err := warmBrowserHTTPModelCodec(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	var indented bytes.Buffer
	if err := json.Indent(&indented, bytes.TrimSpace(fixture), "", "  "); err != nil {
		t.Fatal(err)
	}
	for _, variant := range []struct {
		name  string
		codec runtime.Marshaler
	}{{"upstream_decoder", browserCodecDefaultJSON()}, {"exact_body_cache", codec}} {
		t.Run(variant.name, func(t *testing.T) {
			g := newBrowserCodecGateway(t, variant.codec, true, nil, nil)
			first, second := g.createStore(t), g.createStore(t)
			ids := make([]string, 0, 2)
			for _, store := range []string{first, second} {
				code, response := g.request(t, http.MethodPost, "/stores/"+store+"/authorization-models", bytes.TrimSpace(fixture), browserCodecTestToken)
				if code != http.StatusCreated {
					t.Fatalf("pinned write failed: HTTP %d %s", code, response)
				}
				ids = append(ids, g.assertWrittenModel(t, store, fixture, response))
				g.assertModelCount(t, store, 1)
			}
			if ids[0] == ids[1] {
				t.Fatal("two real writes reused a model ID")
			}
			for i, store := range []string{first, second} {
				if _, err := g.datastore.ReadAuthorizationModel(context.Background(), store, ids[1-i]); !errors.Is(err, storage.ErrNotFound) {
					t.Fatalf("model crossed store boundary: %v", err)
				}
			}
			for _, test := range []struct {
				name, token, store string
				body               []byte
				want               int
			}{
				{"missing_auth", "", first, fixture, http.StatusUnauthorized},
				{"wrong_auth", "wrong-key", first, fixture, http.StatusUnauthorized},
				{"invalid_url_store", browserCodecTestToken, "invalid", fixture, http.StatusBadRequest},
				{"malformed_json", browserCodecTestToken, first, []byte(`{"schema_version":`), http.StatusBadRequest},
				{"mismatching_invalid_schema", browserCodecTestToken, first, []byte(`{"schema_version":"invalid","type_definitions":[{"type":"user"}]}`), http.StatusBadRequest},
				{"mismatching_invalid_model", browserCodecTestToken, first, []byte(`{"schema_version":"1.1","type_definitions":[{"type":"document","relations":{"viewer":{"this":{}}}}]}`), http.StatusBadRequest},
			} {
				t.Run(test.name, func(t *testing.T) {
					code, response := g.request(t, http.MethodPost, "/stores/"+test.store+"/authorization-models", test.body, test.token)
					if code != test.want {
						t.Fatalf("HTTP %d %s; want %d", code, response, test.want)
					}
					g.assertModelCount(t, first, 1)
					g.assertModelCount(t, second, 1)
				})
			}
			for _, test := range []struct {
				name string
				body []byte
			}{
				{"valid_different_model", []byte(`{"schema_version":"1.1","type_definitions":[{"type":"user"}]}`)},
				{"same_model_different_whitespace", indented.Bytes()},
				{"url_overrides_body_store_id", append([]byte(`{"store_id":"`+second+`",`), bytes.TrimSpace(fixture)[1:]...)},
			} {
				t.Run(test.name, func(t *testing.T) {
					code, response := g.request(t, http.MethodPost, "/stores/"+first+"/authorization-models", test.body, browserCodecTestToken)
					if code != http.StatusCreated {
						t.Fatalf("real fallback write failed: HTTP %d %s", code, response)
					}
					g.assertWrittenModel(t, first, test.body, response)
					g.assertModelCount(t, second, 1)
				})
			}
			g.assertModelCount(t, first, 4)
		})
	}
}

func TestBrowserHTTPModelCodecGatewayCancellation(t *testing.T) {
	path, fixture := browserHTTPModelFixture(t)
	codec, err := warmBrowserHTTPModelCodec(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	for _, variant := range []struct {
		name  string
		codec runtime.Marshaler
	}{{"upstream_decoder", browserCodecDefaultJSON()}, {"exact_body_cache", codec}} {
		t.Run(variant.name, func(t *testing.T) {
			entered := make(chan struct{}, 1)
			finished := make(chan error, 1)
			gate := func(ctx context.Context, req interface{}, info *grpc.UnaryServerInfo, next grpc.UnaryHandler) (interface{}, error) {
				if !strings.HasSuffix(info.FullMethod, "/WriteAuthorizationModel") {
					return next(ctx, req)
				}
				entered <- struct{}{}
				// Deterministic scheduling boundary, not a replacement response:
				// after actual HTTP cancellation the real handler still executes.
				<-ctx.Done()
				response, err := next(ctx, req)
				if !errors.Is(ctx.Err(), context.Canceled) {
					finished <- fmt.Errorf("RPC did not inherit HTTP cancellation: %v", ctx.Err())
				} else if err == nil {
					finished <- errors.New("real handler accepted an ended request")
				} else {
					finished <- nil
				}
				return response, err
			}
			g := newBrowserCodecGateway(t, variant.codec, true, gate, nil)
			store := g.createStore(t)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			r, err := http.NewRequestWithContext(ctx, http.MethodPost, g.url+"/stores/"+store+"/authorization-models", bytes.NewReader(bytes.TrimSpace(fixture)))
			if err != nil {
				t.Fatal(err)
			}
			r.Header.Set("Content-Type", "application/json")
			r.Header.Set("Authorization", "Bearer "+browserCodecTestToken)
			clientResult := make(chan error, 1)
			go func() {
				response, err := g.client.Do(r)
				if response != nil {
					_ = response.Body.Close()
				}
				clientResult <- err
			}()
			select {
			case <-entered:
			case <-time.After(2 * time.Second):
				t.Fatal("model request never reached actual RPC")
			}
			cancel()
			select {
			case err := <-clientResult:
				if !errors.Is(err, context.Canceled) {
					t.Fatalf("HTTP cancellation was lost: %v", err)
				}
			case <-time.After(2 * time.Second):
				t.Fatal("HTTP cancellation was not bounded")
			}
			select {
			case err := <-finished:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(2 * time.Second):
				t.Fatal("actual RPC handler did not finish after cancellation")
			}
			g.assertModelCount(t, store, 0)
		})
	}
}

// Optional evidence for the exact retained Rust executable, not a reconstructed
// serde_json model. All bootstrap replies are produced by the real service.
func TestBrowserHTTPModelCodecRetainedRustWire(t *testing.T) {
	binary := os.Getenv("BROWSER_RETAINED_LAB_TOOL")
	if binary == "" {
		t.Skip("BROWSER_RETAINED_LAB_TOOL not supplied; retained executable wire capture not run")
	}
	path, fixture := browserHTTPModelFixture(t)
	codec, err := warmBrowserHTTPModelCodec(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	type observedRequest struct {
		Path          string `json:"path"`
		ContentLength int64  `json:"content_length"`
		body          []byte
	}
	requests := make(chan observedRequest, 4)
	g := newBrowserCodecGateway(t, codec, false, nil, func(r *http.Request, body []byte) {
		requests <- observedRequest{Path: r.URL.Path, ContentLength: r.ContentLength, body: bytes.Clone(body)}
	})
	configPath := filepath.Join(t.TempDir(), "config.json")
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, binary, "bootstrap")
	for _, entry := range os.Environ() {
		if !strings.HasPrefix(entry, "FGA_URL=") && !strings.HasPrefix(entry, "FGA_MODEL=") && !strings.HasPrefix(entry, "FGA_CONFIG=") {
			command.Env = append(command.Env, entry)
		}
	}
	command.Env = append(command.Env, "FGA_URL="+g.url, "FGA_MODEL="+path, "FGA_CONFIG="+configPath)
	output, err := command.CombinedOutput()
	if err != nil {
		t.Fatalf("real retained bootstrap failed: %v\n%s", err, output)
	}
	if len(requests) != 1 {
		t.Fatalf("expected one actual model POST, got %d", len(requests))
	}
	request := <-requests
	if !bytes.Equal(request.body, bytes.TrimSpace(fixture)) || request.ContentLength != int64(len(request.body)) {
		t.Fatalf("actual Rust body misses exact cache bytes: len=%d sha256=%x", len(request.body), sha256.Sum256(request.body))
	}
	configBytes, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatal(err)
	}
	var binding struct {
		StoreID string `json:"store_id"`
		ModelID string `json:"model_id"`
	}
	if err := json.Unmarshal(configBytes, &binding); err != nil {
		t.Fatal(err)
	}
	if request.Path != "/stores/"+binding.StoreID+"/authorization-models" {
		t.Fatal("wire POST path differs from verified real store binding")
	}
	g.assertModelCount(t, binding.StoreID, 1)
	created, err := protojson.Marshal(&openfgav1.WriteAuthorizationModelResponse{AuthorizationModelId: binding.ModelID})
	if err != nil {
		t.Fatal(err)
	}
	g.assertWrittenModel(t, binding.StoreID, request.body, created)
	t.Logf("retained Rust bootstrap succeeded against real memory gateway: body_bytes=%d body_sha256=%x exact_pinned_raw=true", len(request.body), sha256.Sum256(request.body))
	if directory := os.Getenv("BROWSER_MODEL_CODEC_PROOF_DIR"); directory != "" {
		if err := os.MkdirAll(directory, 0o755); err != nil {
			t.Fatal(err)
		}
		binaryBytes, err := os.ReadFile(binary)
		if err != nil {
			t.Fatal(err)
		}
		proof, err := json.MarshalIndent(map[string]interface{}{
			"scope":  "Actual retained Rust bootstrap over HTTP, generated gateway, loopback gRPC and real memory datastore; not PostgreSQL or browser acceptance",
			"binary": binary, "binary_sha256": fmt.Sprintf("%x", sha256.Sum256(binaryBytes)),
			"fixture_sha256": fmt.Sprintf("%x", sha256.Sum256(fixture)), "body_bytes": len(request.body), "body_sha256": fmt.Sprintf("%x", sha256.Sum256(request.body)),
			"body_matches_pinned_raw_json": true, "request": request, "binding": binding, "bootstrap_exit_code": 0, "bootstrap_output": string(output),
		}, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		for name, data := range map[string][]byte{"actual-rust-body.json": request.body, "wire-proof.json": append(proof, '\n'), "bootstrap-output.txt": output} {
			if err := os.WriteFile(filepath.Join(directory, name), data, 0o644); err != nil {
				t.Fatal(err)
			}
		}
	}
}
