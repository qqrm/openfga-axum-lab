package run

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/openfga/openfga/pkg/logger"
	serverconfig "github.com/openfga/openfga/pkg/server/config"
)

func browserHTTPModelFixture(t *testing.T) (string, []byte) {
	t.Helper()
	path := os.Getenv("BROWSER_OPENFGA_TEST_MODEL")
	if path == "" {
		t.Fatal("BROWSER_OPENFGA_TEST_MODEL must name the repository's pinned fga/model.json")
	}
	data, err := readBrowserHTTPModel(path)
	if err != nil {
		t.Fatal(err)
	}
	return path, data
}

func TestBrowserHTTPWarmupExactModel(t *testing.T) {
	path, original := browserHTTPModelFixture(t)
	if err := warmBrowserHTTPModel(context.Background(), path); err != nil {
		t.Fatal(err)
	}
	after, err := os.ReadFile(path)
	if err != nil || !bytes.Equal(original, after) {
		t.Fatalf("warmup changed the pinned file: %v", err)
	}
}

func TestBrowserHTTPWarmupRejectsModel(t *testing.T) {
	_, original := browserHTTPModelFixture(t)
	tampered := bytes.Clone(original)
	// A same-length, still-valid JSON whitespace change must fail identity.
	tampered[len(tampered)-1] = ' '
	for _, test := range []struct {
		name string
		data []byte
		want string
	}{
		{"tampered", tampered, "checksum mismatch"},
		{"oversized", append(bytes.Clone(original), ' '), "model size"},
		{"truncated", original[:len(original)-1], "model size"},
	} {
		t.Run(test.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "model.json")
			if err := os.WriteFile(path, test.data, 0600); err != nil {
				t.Fatal(err)
			}
			err := warmBrowserHTTPModel(context.Background(), path)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("untrusted model accepted or wrong failure: %v", err)
			}
		})
	}
	if err := warmBrowserHTTPModel(context.Background(), filepath.Join(t.TempDir(), "missing.json")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("missing model did not fail closed: %v", err)
	}
	if err := warmBrowserHTTPModel(context.Background(), t.TempDir()); err == nil || !strings.Contains(err.Error(), "regular file") {
		t.Fatalf("non-regular model did not fail closed: %v", err)
	}
}

func TestBrowserHTTPWarmupBoundedWorker(t *testing.T) {
	// A codec/validation call may ignore cancellation until it returns. The
	// production wrapper must return its own deadline without waiting for it.
	release := make(chan struct{})
	entered := make(chan struct{})
	finished := make(chan struct{})
	defer func() {
		close(release)
		select {
		case <-finished:
		case <-time.After(time.Second):
			t.Error("test worker did not finish after release")
		}
	}()
	started := time.Now()
	err := runBrowserHTTPWarmup(context.Background(), 20*time.Millisecond, func(context.Context) error {
		defer close(finished)
		close(entered)
		<-release
		return nil
	})
	if !errors.Is(err, context.DeadlineExceeded) || time.Since(started) > time.Second {
		t.Fatalf("non-cooperative worker escaped the startup bound: %v", err)
	}
	select {
	case <-entered:
	default:
		t.Fatal("test worker did not start")
	}
	select {
	case <-finished:
		t.Fatal("test did not leave synchronous work running beyond the deadline")
	default:
	}
}

func TestBrowserHTTPWarmupParentCancellation(t *testing.T) {
	path, _ := browserHTTPModelFixture(t)
	cancelled, cancelNow := context.WithCancel(context.Background())
	cancelNow()
	expired, cancelExpired := context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
	defer cancelExpired()
	for _, test := range []struct {
		name string
		ctx  context.Context
		want error
	}{
		{"cancelled", cancelled, context.Canceled},
		{"deadline", expired, context.DeadlineExceeded},
	} {
		t.Run(test.name, func(t *testing.T) {
			if err := warmBrowserHTTPModel(test.ctx, path); !errors.Is(err, test.want) {
				t.Fatalf("parent cancellation lost: %v", err)
			}
		})
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := runBrowserHTTPWarmup(ctx, time.Second, func(context.Context) error {
		cancel()
		return nil
	}); !errors.Is(err, context.Canceled) {
		t.Fatalf("late success won over parent cancellation: %v", err)
	}
}

func TestBrowserHTTPWarmupPreventsServerStartup(t *testing.T) {
	// Exercise the patched Run entry point. Invalid datastore/listener settings
	// must remain unreachable when the model or parent context is unacceptable.
	path, _ := browserHTTPModelFixture(t)
	for _, cancelled := range []bool{false, true} {
		t.Run(map[bool]string{false: "missing_model", true: "cancelled_parent"}[cancelled], func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			want := error(os.ErrNotExist)
			fixture := filepath.Join(t.TempDir(), "missing.json")
			if cancelled {
				cancel()
				fixture, want = path, context.Canceled
			}
			t.Setenv("FGA_MODEL", fixture)
			cfg := serverconfig.DefaultConfig()
			cfg.HTTP.Enabled = true
			cfg.Datastore.Engine = "must-not-be-initialized"
			cfg.HTTP.Addr, cfg.GRPC.Addr = "invalid-http-address", "invalid-grpc-address"
			server := &ServerContext{Logger: logger.NewNoopLogger()}
			err := server.Run(ctx, cfg)
			if !errors.Is(err, want) || !strings.Contains(err.Error(), "browser model warmup failed") {
				t.Fatalf("server passed the warmup readiness gate: %v", err)
			}
		})
	}
}
