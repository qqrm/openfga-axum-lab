package run

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
	"google.golang.org/grpc/status"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"

	fgaserver "github.com/openfga/openfga/pkg/server"
	serverconfig "github.com/openfga/openfga/pkg/server/config"
	"github.com/openfga/openfga/pkg/storage"
	"github.com/openfga/openfga/pkg/storage/memory"
	"github.com/openfga/openfga/pkg/typesystem"
)

func browserDialOptions() []grpc.DialOption {
	// Match the production plaintext loopback connection, including WithBlock.
	// nolint:staticcheck // This regression test preserves upstream dial options.
	return []grpc.DialOption{grpc.WithBlock(), grpc.WithTransportCredentials(insecure.NewCredentials())}
}

// The socket exists immediately, but the real gRPC server cannot finish its
// HTTP/2 handshake until delay elapses. No successful response is fabricated.
func delayedBrowserGRPCServer(t *testing.T, delay time.Duration) string {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := grpc.NewServer()
	healthServer := health.NewServer()
	healthServer.SetServingStatus("", healthpb.HealthCheckResponse_SERVING)
	healthpb.RegisterHealthServer(server, healthServer)
	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		timer := time.NewTimer(delay)
		defer timer.Stop()
		select {
		case <-stop:
			return
		case <-timer.C:
			_ = server.Serve(listener)
		}
	}()
	t.Cleanup(func() {
		close(stop)
		server.Stop()
		_ = listener.Close()
		<-done
	})
	return listener.Addr().String()
}

func TestBrowserHTTPDialSlowStartup(t *testing.T) {
	t.Run("upstream_three_second_budget_rejects_slow_handshake", func(t *testing.T) {
		address := delayedBrowserGRPCServer(t, 4*time.Second)
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		// This is the original call's three-second negative control.
		// nolint:staticcheck // Compare with the pinned upstream implementation.
		conn, err := grpc.DialContext(ctx, address, browserDialOptions()...)
		if conn != nil {
			_ = conn.Close()
			t.Fatal("a connection must not be accepted before the handshake")
		}
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("want deadline exceeded, got %v", err)
		}
	})
	t.Run("browser_startup_waits_for_the_real_handshake", func(t *testing.T) {
		address := delayedBrowserGRPCServer(t, 4*time.Second)
		conn, err := dialBrowserHTTPGRPC(context.Background(), address, browserDialOptions()...)
		if err != nil {
			t.Fatal(err)
		}
		defer conn.Close()
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		response, err := healthpb.NewHealthClient(conn).Check(ctx, &healthpb.HealthCheckRequest{})
		if err != nil || response.GetStatus() != healthpb.HealthCheckResponse_SERVING {
			t.Fatalf("real gRPC health request failed: response=%v err=%v", response, err)
		}
	})
}

func TestBrowserHTTPDialUnavailableIsBounded(t *testing.T) {
	// The server never accepts within the startup budget. Test the production
	// helper's own 60-second limit, rather than a shorter test-only replica.
	address := delayedBrowserGRPCServer(t, 24*time.Hour)
	started := time.Now()
	conn, err := dialBrowserHTTPGRPC(context.Background(), address, browserDialOptions()...)
	if conn != nil {
		_ = conn.Close()
		t.Fatal("unavailable gRPC server returned a connection")
	}
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("want deadline exceeded, got %v", err)
	}
	if elapsed := time.Since(started); elapsed < 60*time.Second || elapsed > 70*time.Second {
		t.Fatalf("startup dial escaped its 60-second bound: %s", elapsed)
	}
}

func TestBrowserHTTPDialParentCancellation(t *testing.T) {
	address := delayedBrowserGRPCServer(t, 24*time.Hour)
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	started := time.Now()
	conn, err := dialBrowserHTTPGRPC(ctx, address, browserDialOptions()...)
	if conn != nil {
		_ = conn.Close()
		t.Fatal("cancelled startup returned a connection")
	}
	if !errors.Is(err, context.DeadlineExceeded) || time.Since(started) > time.Second {
		t.Fatalf("parent deadline did not stop startup: %v", err)
	}
	cancelled, cancelNow := context.WithCancel(context.Background())
	cancelNow()
	conn, err = dialBrowserHTTPGRPC(cancelled, address, browserDialOptions()...)
	if conn != nil {
		_ = conn.Close()
		t.Fatal("cancelled startup returned a connection")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("parent cancellation was lost: %v", err)
	}
}

func TestBrowserHTTPDialAuthorizationDefaultsUnchanged(t *testing.T) {
	cfg := serverconfig.DefaultConfig()
	if cfg.RequestTimeout != 3*time.Second || cfg.HTTP.UpstreamTimeout != 5*time.Second ||
		cfg.ListObjectsDeadline != 3*time.Second || cfg.ListUsersDeadline != 3*time.Second ||
		serverconfig.DefaultContextTimeout(cfg) != 6*time.Second {
		t.Fatal("browser startup adaptation changed authorization request deadlines")
	}
}

func TestBrowserHTTPTracePreservesHandler(t *testing.T) {
	for _, code := range []int{http.StatusCreated, http.StatusServiceUnavailable} {
		t.Run(http.StatusText(code), func(t *testing.T) {
			var logs bytes.Buffer
			output := log.New(&logs, "", 0)
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			request := httptest.NewRequest(http.MethodPost, "/stores?private-query", strings.NewReader("private-request"))
			request = request.WithContext(ctx)
			request.Header.Set("Authorization", "private-header")
			body := request.Body
			writer := httptest.NewRecorder()
			calls := 0
			handler := http.HandlerFunc(func(actualWriter http.ResponseWriter, actualRequest *http.Request) {
				calls++
				if actualWriter != writer || actualRequest != request || actualRequest.Body != body || actualRequest.Context() != ctx {
					t.Fatal("observer replaced the writer, request, body or context")
				}
				data, err := io.ReadAll(actualRequest.Body)
				if err != nil || string(data) != "private-request" {
					t.Fatalf("observer consumed or changed the body: %q, %v", data, err)
				}
				actualWriter.Header().Set("X-Original-Handler", "unchanged")
				actualWriter.WriteHeader(code)
				_, _ = actualWriter.Write([]byte("private-response"))
				if code == http.StatusServiceUnavailable {
					cancel()
				}
			})
			browserHTTPObserver(handler, output).ServeHTTP(writer, request)
			if calls != 1 || writer.Code != code || writer.Body.String() != "private-response" || writer.Header().Get("X-Original-Handler") != "unchanged" {
				t.Fatal("observer changed the handler call or HTTP result")
			}
			text := logs.String()
			for _, expected := range []string{"http_start method=\"POST\" path=\"/stores\" content_length=15", "http_end method=\"POST\" path=\"/stores\" content_length=15 duration_ms="} {
				if !strings.Contains(text, expected) {
					t.Fatalf("missing HTTP boundary metadata %q in %s", expected, text)
				}
			}
			contextError := "context_error=<nil>"
			if code == http.StatusServiceUnavailable {
				contextError = "context_error=context canceled"
			}
			if !strings.Contains(text, contextError) || strings.Count(text, "\n") != 2 || strings.Contains(text, "private-") {
				t.Fatalf("HTTP diagnostics lost context, duplicated events or disclosed payload: %s", text)
			}
		})
	}
}

func TestBrowserHTTPTracePreservesRPC(t *testing.T) {
	for _, test := range []struct {
		name string
		code codes.Code
		err  error
	}{
		{"OK", codes.OK, nil},
		{"PermissionDenied", codes.PermissionDenied, status.Error(codes.PermissionDenied, "private-error")},
		{"StatusDeadline", codes.DeadlineExceeded, status.Error(codes.DeadlineExceeded, "private-error")},
		{"ContextCanceled", codes.Canceled, context.Canceled},
		{"ContextDeadline", codes.DeadlineExceeded, context.DeadlineExceeded},
		{"WrappedCanceled", codes.Canceled, fmt.Errorf("private-error: %w", context.Canceled)},
		{"WrappedDeadline", codes.DeadlineExceeded, fmt.Errorf("private-error: %w", context.DeadlineExceeded)},
	} {
		t.Run(test.name, func(t *testing.T) {
			var logs bytes.Buffer
			output := log.New(&logs, "", 0)
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			request := &healthpb.HealthCheckRequest{Service: "private-request"}
			response := &healthpb.HealthCheckResponse{Status: healthpb.HealthCheckResponse_SERVING}
			expectedError := test.err
			info := &grpc.UnaryServerInfo{FullMethod: "/openfga.v1.OpenFGAService/CreateStore"}
			calls := 0
			handler := func(actualContext context.Context, actualRequest interface{}) (interface{}, error) {
				calls++
				if actualContext != ctx || actualRequest != request {
					t.Fatal("observer replaced the RPC context or request")
				}
				return response, expectedError
			}
			actualResponse, actualError := browserUnaryObserver(output)(ctx, request, info, handler)
			if calls != 1 || actualResponse != response || actualError != expectedError {
				t.Fatal("observer changed the handler call, response or error")
			}
			text := logs.String()
			errorKind := "other"
			if test.err == nil {
				errorKind = "none"
			}
			if !strings.Contains(text, "rpc_start method=\""+info.FullMethod+"\"") ||
				!strings.Contains(text, "rpc_end method=\""+info.FullMethod+"\" status=\""+test.code.String()+"\" duration_ms=") ||
				!strings.Contains(text, "context_state=\"none\" error_kind=\""+errorKind+"\"") ||
				strings.Count(text, "\n") != 2 || strings.Contains(text, "private-") {
				t.Fatalf("RPC diagnostics changed status or disclosed payload: %s", text)
			}
		})
	}
}

func TestBrowserHTTPTraceClassifiesOnlyAllowlistedErrors(t *testing.T) {
	const deadlineMessage = "invalid authorization model encountered: context deadline exceeded"
	const canceledMessage = "invalid authorization model encountered: context canceled"
	validationCode := codes.Code(openfgav1.ErrorCode_validation_error)
	for _, test := range []struct {
		name string
		err  error
		kind string
	}{
		{"exact_deadline", status.Error(validationCode, deadlineMessage), "model_resolution_deadline"},
		{"exact_canceled", status.Error(validationCode, canceledMessage), "model_resolution_canceled"},
		{"wrong_code", status.Error(codes.DeadlineExceeded, deadlineMessage), "other"},
		{"wrong_application_code", status.Error(codes.Code(openfgav1.InternalErrorCode_deadline_exceeded), deadlineMessage), "other"},
		{"plain_message", errors.New(deadlineMessage), "other"},
		{"prefix", status.Error(validationCode, "private-prefix: "+deadlineMessage), "other"},
		{"suffix", status.Error(validationCode, deadlineMessage+": private-suffix"), "other"},
		{"whitespace", status.Error(validationCode, deadlineMessage+" "), "other"},
		{"newline", status.Error(validationCode, canceledMessage+"\nprivate-message"), "other"},
		{"different_case", status.Error(validationCode, strings.ToUpper(deadlineMessage)), "other"},
		{"other_validation", status.Error(validationCode, "private-model-validation-error"), "other"},
		{"wrapped_status", fmt.Errorf("private-wrapper: %w", status.Error(validationCode, deadlineMessage)), "other"},
	} {
		t.Run(test.name, func(t *testing.T) {
			var logs bytes.Buffer
			ctx := context.Background()
			request := &openfgav1.WriteRequest{StoreId: "private-request"}
			response := &openfgav1.WriteResponse{}
			actualResponse, actualError := browserUnaryObserver(log.New(&logs, "", 0))(ctx, request,
				&grpc.UnaryServerInfo{FullMethod: "/openfga.v1.OpenFGAService/Write"},
				func(actualContext context.Context, actualRequest interface{}) (interface{}, error) {
					if actualContext != ctx || actualRequest != request {
						t.Fatal("observer replaced context or request")
					}
					return response, test.err
				})
			if actualResponse != response || actualError != test.err {
				t.Fatal("classification changed the response or error")
			}
			text := logs.String()
			if !strings.Contains(text, "context_state=\"none\" error_kind=\""+test.kind+"\"") ||
				strings.Count(text, "\n") != 2 || strings.Contains(text, "private-") ||
				strings.Contains(text, "authorization model") || strings.Contains(text, "AUTHORIZATION MODEL") {
				t.Fatalf("classification is not exact or disclosed an error message: %s", text)
			}
		})
	}
	if browserRPCContextState(errors.New("private-context-error")) != "other" {
		t.Fatal("unexpected context error did not remain a fixed redacted label")
	}
}

// Exercise the real memory datastore, resolver and server error mapping. An
// already-ended request is deterministic and does not claim to reproduce the
// browser's scheduling delay; a live request proves this model/tuple is valid.
func TestBrowserHTTPTraceMemoryResolverContextFailure(t *testing.T) {
	for _, ended := range []string{"deadline", "canceled"} {
		t.Run(ended, func(t *testing.T) {
			const storeID = "01JCC8Z5S039R3X661KQGTNAFG"
			const modelID = "01JCC8ZD4X84K2W0H0ZA5AQ947"
			datastore := memory.New()
			t.Cleanup(datastore.Close)
			_, err := datastore.CreateStore(context.Background(), &openfgav1.Store{Id: storeID, Name: "browser-trace-test"})
			if err != nil {
				t.Fatal(err)
			}
			model := &openfgav1.AuthorizationModel{
				Id: modelID, SchemaVersion: typesystem.SchemaVersion1_1,
				TypeDefinitions: []*openfgav1.TypeDefinition{
					{Type: "user"},
					{
						Type: "document", Relations: map[string]*openfgav1.Userset{"viewer": typesystem.This()},
						Metadata: &openfgav1.Metadata{Relations: map[string]*openfgav1.RelationMetadata{
							"viewer": {DirectlyRelatedUserTypes: []*openfgav1.RelationReference{typesystem.DirectRelationReference("user", "")}},
						}},
					},
				},
			}
			if err := datastore.WriteAuthorizationModel(context.Background(), storeID, model); err != nil {
				t.Fatal(err)
			}
			server, err := fgaserver.NewServerWithOpts(fgaserver.WithDatastore(datastore))
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(server.Close)
			key := &openfgav1.TupleKey{Object: "document:1", Relation: "viewer", User: "user:1"}
			request := &openfgav1.WriteRequest{
				StoreId: storeID, AuthorizationModelId: modelID,
				Writes: &openfgav1.WriteRequestWrites{TupleKeys: []*openfgav1.TupleKey{key}},
			}
			ctx, cancel := context.WithCancel(context.Background())
			cancel()
			if ended == "deadline" {
				ctx, cancel = context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
				defer cancel()
			}
			var logs bytes.Buffer
			var originalResponse *openfgav1.WriteResponse
			var originalError error
			actualResponse, actualError := browserUnaryObserver(log.New(&logs, "", 0))(ctx, request,
				&grpc.UnaryServerInfo{FullMethod: "/openfga.v1.OpenFGAService/Write"},
				func(actualContext context.Context, actualRequest interface{}) (interface{}, error) {
					if actualContext != ctx || actualRequest != request {
						t.Fatal("observer replaced context or request")
					}
					originalResponse, originalError = server.Write(actualContext, actualRequest.(*openfgav1.WriteRequest))
					return originalResponse, originalError
				})
			if actualResponse != originalResponse || actualError != originalError || originalResponse != nil {
				t.Fatal("observer changed the failed server response or error")
			}
			expectedMessage := "invalid authorization model encountered: " + ctx.Err().Error()
			if status.Code(actualError) != codes.Code(openfgav1.ErrorCode_validation_error) || status.Convert(actualError).Message() != expectedMessage {
				t.Fatalf("real resolver no longer maps context failure to the allowlisted status: %v", actualError)
			}
			text := logs.String()
			if !strings.Contains(text, "context_state=\""+ended+"\" error_kind=\"model_resolution_"+ended+"\"") ||
				strings.Contains(text, expectedMessage) || strings.Count(text, "\n") != 2 {
				t.Fatalf("real resolver failure was not classified without its message: %s", text)
			}
			if _, err := datastore.ReadUserTuple(context.Background(), storeID, key, storage.ReadUserTupleOptions{}); !errors.Is(err, storage.ErrNotFound) {
				t.Fatalf("ended request wrote a tuple: %v", err)
			}
			if _, err := server.Write(context.Background(), request); err != nil {
				t.Fatalf("live request with the same model and tuple did not succeed: %v", err)
			}
			if _, err := datastore.ReadUserTuple(context.Background(), storeID, key, storage.ReadUserTupleOptions{}); err != nil {
				t.Fatalf("successful live request did not write its real tuple: %v", err)
			}
		})
	}
}

func TestBrowserHTTPTracePreservesPanics(t *testing.T) {
	for _, transport := range []string{"http", "rpc"} {
		t.Run(transport, func(t *testing.T) {
			var logs bytes.Buffer
			output := log.New(&logs, "", 0)
			original := errors.New("private-panic")
			defer func() {
				if recover() != original {
					t.Error("observer intercepted or changed the panic")
				}
				if strings.Count(logs.String(), "\n") != 1 || !strings.Contains(logs.String(), transport+"_start") || strings.Contains(logs.String(), "private-") {
					t.Errorf("panic produced misleading completion or disclosed payload: %s", logs.String())
				}
			}()
			if transport == "http" {
				browserHTTPObserver(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
					panic(original)
				}), output).ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, "/healthz", nil))
			} else {
				_, _ = browserUnaryObserver(output)(context.Background(), nil, &grpc.UnaryServerInfo{FullMethod: "/grpc.health.v1.Health/Check"}, func(context.Context, interface{}) (interface{}, error) {
					panic(original)
				})
			}
			t.Fatal("handler panic was swallowed")
		})
	}
}
