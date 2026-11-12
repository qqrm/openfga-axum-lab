package run

import (
	"context"
	"log"
	"net/http"
	"os"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/status"
)

// Only the browser build uses this bounded startup budget. RPC deadlines remain
// governed by the unchanged upstream configuration and client timeouts.
const browserHTTPGRPCDialTimeout = 60 * time.Second

var browserTransportLogger = log.New(os.Stderr, "[browser-openfga] ", log.LstdFlags|log.Lmicroseconds|log.LUTC)

func dialBrowserHTTPGRPC(ctx context.Context, target string, opts ...grpc.DialOption) (*grpc.ClientConn, error) {
	timeoutCtx, cancel := context.WithTimeout(ctx, browserHTTPGRPCDialTimeout)
	defer cancel()
	started := time.Now()
	browserTransportLogger.Printf("grpc_dial_start timeout_seconds=%d", int(browserHTTPGRPCDialTimeout/time.Second))

	// nolint:staticcheck // Preserve the upstream blocking connection semantics.
	conn, err := grpc.DialContext(timeoutCtx, target, opts...)
	browserTransportLogger.Printf("grpc_dial_end duration_ms=%d failed=%t context_error=%v", time.Since(started).Milliseconds(), err != nil, timeoutCtx.Err())
	return conn, err
}

// Observe transport boundaries without reading bodies or wrapping the writer.
// Completion is logged only when the original handler returns; panics retain
// the upstream recovery path and cannot produce a misleading success event.
func browserHTTPObserver(next http.Handler, output *log.Logger) http.Handler {
	return http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		started := time.Now()
		method, path, length, ctx := request.Method, request.URL.Path, request.ContentLength, request.Context()
		output.Printf("http_start method=%q path=%q content_length=%d", method, path, length)
		next.ServeHTTP(writer, request)
		output.Printf("http_end method=%q path=%q content_length=%d duration_ms=%d context_error=%v", method, path, length, time.Since(started).Milliseconds(), ctx.Err())
	})
}

func browserUnaryObserver(output *log.Logger) grpc.UnaryServerInterceptor {
	return func(ctx context.Context, request interface{}, info *grpc.UnaryServerInfo, next grpc.UnaryHandler) (interface{}, error) {
		started := time.Now()
		output.Printf("rpc_start method=%q", info.FullMethod)
		response, err := next(ctx, request)
		rpcStatus, ok := status.FromError(err)
		if !ok {
			// Match grpc-go's wire status for plain or wrapped context errors.
			rpcStatus = status.FromContextError(err)
		}
		output.Printf("rpc_end method=%q status=%q duration_ms=%d", info.FullMethod, rpcStatus.Code().String(), time.Since(started).Milliseconds())
		return response, err
	}
}
