package run

import (
	"context"
	"time"

	"google.golang.org/grpc"
)

// Only the browser build uses this bounded startup budget. RPC deadlines remain
// governed by the unchanged upstream configuration and client timeouts.
const browserHTTPGRPCDialTimeout = 60 * time.Second

func dialBrowserHTTPGRPC(ctx context.Context, target string, opts ...grpc.DialOption) (*grpc.ClientConn, error) {
	timeoutCtx, cancel := context.WithTimeout(ctx, browserHTTPGRPCDialTimeout)
	defer cancel()

	// nolint:staticcheck // Preserve the upstream blocking connection semantics.
	return grpc.DialContext(timeoutCtx, target, opts...)
}
