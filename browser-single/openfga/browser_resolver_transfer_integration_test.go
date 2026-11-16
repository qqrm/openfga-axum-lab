package server

import (
	"context"
	"crypto/sha256"
	"fmt"
	"os"
	"testing"
	"time"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"github.com/stretchr/testify/require"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	"github.com/openfga/openfga/pkg/storage"
	"github.com/openfga/openfga/pkg/storage/memory"
	"github.com/openfga/openfga/pkg/typesystem"
)

func browserResolverTransferFixture(t *testing.T) *openfgav1.AuthorizationModel {
	t.Helper()
	path := os.Getenv("BROWSER_OPENFGA_TEST_MODEL")
	require.NotEmpty(t, path)
	data, err := os.ReadFile(path)
	require.NoError(t, err)
	require.Len(t, data, 52709)
	require.Equal(t, "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d", fmt.Sprintf("%x", sha256.Sum256(data)))
	model := &openfgav1.AuthorizationModel{}
	require.NoError(t, protojson.Unmarshal(data, model))
	return model
}

func TestBrowserResolverTransferServerPolicy(t *testing.T) {
	// Keep the baseline first and use a fresh real datastore/server for each
	// case. No mocked Check, tuple write, schema validator or datastore result
	// participates in this proof. The typesystem tests separately assert the
	// single claim, mandatory reads, cancellation and concurrent-store binding.
	var baselineErrors []codes.Code
	for _, transfer := range []bool{false, true} {
		t.Run(fmt.Sprintf("startup_transfer_%t", transfer), func(t *testing.T) {
			model := browserResolverTransferFixture(t)
			if transfer {
				prepared, err := typesystem.PrepareBrowserModelValidation(context.Background(), model)
				require.NoError(t, err)
				require.NoError(t, typesystem.InstallBrowserModelValidation(context.Background(), prepared))
			}
			datastore := memory.New()
			s, err := NewServerWithOpts(WithDatastore(datastore))
			require.NoError(t, err)
			t.Cleanup(s.Close)
			requestContext := func() context.Context {
				ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
				t.Cleanup(cancel)
				return ctx
			}

			stores := make([]string, 2)
			models := make([]string, 2)
			for i := range stores {
				created, err := s.CreateStore(requestContext(), &openfgav1.CreateStoreRequest{Name: fmt.Sprintf("browser-transfer-%d", i)})
				require.NoError(t, err)
				stores[i] = created.GetId()
				written, err := s.WriteAuthorizationModel(requestContext(), &openfgav1.WriteAuthorizationModelRequest{
					StoreId:         stores[i],
					SchemaVersion:   model.GetSchemaVersion(),
					TypeDefinitions: proto.Clone(model).(*openfgav1.AuthorizationModel).GetTypeDefinitions(),
					Conditions:      model.GetConditions(),
				})
				require.NoError(t, err)
				models[i] = written.GetAuthorizationModelId()
				require.NotEmpty(t, models[i])

				read, err := s.ReadAuthorizationModel(requestContext(), &openfgav1.ReadAuthorizationModelRequest{StoreId: stores[i], Id: models[i]})
				require.NoError(t, err)
				want := proto.Clone(model).(*openfgav1.AuthorizationModel)
				want.Id = models[i]
				require.True(t, proto.Equal(want, read.GetAuthorizationModel()))
			}
			require.NotEqual(t, stores[0], stores[1])
			require.NotEqual(t, models[0], models[1])

			grant := &openfgav1.TupleKey{Object: "lakekeeper_table:browser-transfer", Relation: "select", User: "user:browser-transfer"}
			writeRequest := func(store, modelID string, tuple *openfgav1.TupleKey) *openfgav1.WriteRequest {
				return &openfgav1.WriteRequest{
					StoreId: store, AuthorizationModelId: modelID,
					Writes: &openfgav1.WriteRequestWrites{TupleKeys: []*openfgav1.TupleKey{tuple}},
				}
			}
			var observedErrors []codes.Code

			// A valid ID from another real store must not resolve or write.
			response, err := s.Write(requestContext(), writeRequest(stores[1], models[0], proto.Clone(grant).(*openfgav1.TupleKey)))
			require.Error(t, err)
			require.Nil(t, response)
			observedErrors = append(observedErrors, status.Code(err))

			// This is the first successful resolver consumer. Model publication
			// and the readbacks above must leave the startup object available.
			response, err = s.Write(requestContext(), writeRequest(stores[0], models[0], proto.Clone(grant).(*openfgav1.TupleKey)))
			require.NoError(t, err)
			require.NotNil(t, response)
			stored, err := datastore.ReadUserTuple(requestContext(), stores[0], grant, storage.ReadUserTupleOptions{})
			require.NoError(t, err)
			require.True(t, proto.Equal(grant, stored.GetKey()))

			check := func(store, modelID, user string, want bool) {
				t.Helper()
				decision, err := s.Check(requestContext(), &openfgav1.CheckRequest{
					StoreId: store, AuthorizationModelId: modelID,
					TupleKey: &openfgav1.CheckRequestTupleKey{
						Object: grant.GetObject(), Relation: "can_read_data", User: user,
					},
					Consistency: openfgav1.ConsistencyPreference_HIGHER_CONSISTENCY,
				})
				require.NoError(t, err)
				require.Equal(t, want, decision.GetAllowed())
			}
			check(stores[0], models[0], grant.GetUser(), true)
			check(stores[0], models[0], "user:browser-other", false)
			check(stores[1], models[1], grant.GetUser(), false)

			// Preserve both schema validation and real datastore errors:
			// computed relations cannot be directly granted, and a duplicate
			// tuple is still rejected by the real write path.
			invalid := proto.Clone(grant).(*openfgav1.TupleKey)
			invalid.Relation = "can_read_data"
			invalid.User = "user:browser-other"
			response, err = s.Write(requestContext(), writeRequest(stores[0], models[0], invalid))
			require.Error(t, err)
			require.Nil(t, response)
			observedErrors = append(observedErrors, status.Code(err))
			check(stores[0], models[0], invalid.GetUser(), false)
			response, err = s.Write(requestContext(), writeRequest(stores[0], models[0], proto.Clone(grant).(*openfgav1.TupleKey)))
			require.Error(t, err)
			require.Nil(t, response)
			observedErrors = append(observedErrors, status.Code(err))

			deleted, err := s.Write(requestContext(), &openfgav1.WriteRequest{
				StoreId: stores[0], AuthorizationModelId: models[0],
				Deletes: &openfgav1.WriteRequestDeletes{TupleKeys: []*openfgav1.TupleKeyWithoutCondition{{
					Object: grant.GetObject(), Relation: grant.GetRelation(), User: grant.GetUser(),
				}}},
			})
			require.NoError(t, err)
			require.NotNil(t, deleted)
			_, err = datastore.ReadUserTuple(requestContext(), stores[0], grant, storage.ReadUserTupleOptions{})
			require.ErrorIs(t, err, storage.ErrNotFound)
			check(stores[0], models[0], grant.GetUser(), false)
			if transfer {
				require.Equal(t, baselineErrors, observedErrors)
			} else {
				baselineErrors = observedErrors
			}
		})
	}
}
