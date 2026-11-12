package run

import (
	"context"
	"errors"
	"net"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"

	serverconfig "github.com/openfga/openfga/pkg/server/config"
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
