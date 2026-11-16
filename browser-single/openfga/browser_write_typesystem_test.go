package commands

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/proto"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	parser "github.com/openfga/language/pkg/go/transformer"

	serverErrors "github.com/openfga/openfga/pkg/server/errors"
	"github.com/openfga/openfga/pkg/storage"
	"github.com/openfga/openfga/pkg/storage/memory"
	"github.com/openfga/openfga/pkg/testutils"
	"github.com/openfga/openfga/pkg/tuple"
	"github.com/openfga/openfga/pkg/typesystem"
)

const (
	browserWriteStore = "01JCC8Z5S039R3X661KQGTNAFG"
	browserOtherStore = "01JCC8Z5S039R3X661KQGTNAFH"
	browserWriteModel = "01JCC8ZD4X84K2W0H0ZA5AQ947"
	browserOtherModel = "01JCC8ZD4X84K2W0H0ZA5AQ948"
)

// Observe calls, but use the real in-memory backend for model/tuple storage and
// its duplicate-write errors. The context check models the SQL write contract;
// the in-memory backend itself does not check cancellation before writing.
type browserWriteDatastore struct {
	storage.OpenFGADatastore
	reads        atomic.Int64
	writes       atomic.Int64
	writeError   error
	inspectWrite func(context.Context)
}

func (d *browserWriteDatastore) ReadAuthorizationModel(ctx context.Context, store, model string) (*openfgav1.AuthorizationModel, error) {
	d.reads.Add(1)
	return d.OpenFGADatastore.ReadAuthorizationModel(ctx, store, model)
}

func (d *browserWriteDatastore) Write(ctx context.Context, store string, deletes storage.Deletes, writes storage.Writes, opts ...storage.TupleWriteOption) error {
	d.writes.Add(1)
	if d.inspectWrite != nil {
		d.inspectWrite(ctx)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if d.writeError != nil {
		return d.writeError
	}
	return d.OpenFGADatastore.Write(ctx, store, deletes, writes, opts...)
}

func newBrowserWriteFixture(t *testing.T) (*browserWriteDatastore, *typesystem.TypeSystem) {
	t.Helper()
	backend := memory.New(memory.WithMaxTuplesPerWrite(2))
	t.Cleanup(backend.Close)
	model := parser.MustTransformDSLToProto(`
model
  schema 1.1
type user
type group
  relations
    define member: [user]
type document
  relations
    define viewer: [user]
    define computed: viewer
`)
	model.Id = browserWriteModel
	typesys, err := typesystem.NewAndValidate(context.Background(), model)
	require.NoError(t, err)
	require.NoError(t, backend.WriteAuthorizationModel(context.Background(), browserWriteStore, model))
	restricted := proto.Clone(model).(*openfgav1.AuthorizationModel)
	restricted.TypeDefinitions[2].Metadata.Relations["viewer"].DirectlyRelatedUserTypes = []*openfgav1.RelationReference{
		typesystem.DirectRelationReference("group", "member"),
	}
	require.NoError(t, backend.WriteAuthorizationModel(context.Background(), browserOtherStore, restricted))
	restricted = proto.Clone(restricted).(*openfgav1.AuthorizationModel)
	restricted.Id = browserOtherModel
	require.NoError(t, backend.WriteAuthorizationModel(context.Background(), browserWriteStore, restricted))
	return &browserWriteDatastore{OpenFGADatastore: backend}, typesys
}

func browserWriteRequest(keys ...*openfgav1.TupleKey) *openfgav1.WriteRequest {
	return &openfgav1.WriteRequest{
		StoreId: browserWriteStore, AuthorizationModelId: browserWriteModel,
		Writes: &openfgav1.WriteRequestWrites{TupleKeys: keys},
	}
}

func TestBrowserWriteResolvedTypesystemBinding(t *testing.T) {
	for _, tc := range []struct {
		name, store, model string
		nilTypesystem      bool
		wantReads          int64
		wantError          bool
	}{
		{"matching_binding", browserWriteStore, browserWriteModel, false, 0, false},
		{"nil_uses_original_path", browserWriteStore, browserWriteModel, true, 1, false},
		{"wrong_store_same_model_id", browserOtherStore, browserWriteModel, false, 1, true},
		{"wrong_model_same_store", browserWriteStore, browserOtherModel, false, 1, true},
		{"empty_model_uses_latest_original_path", browserWriteStore, "", false, 1, true},
		{"missing_model", browserWriteStore, "01JCC8ZD4X84K2W0H0ZA5AQ949", false, 1, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ds, resolved := newBrowserWriteFixture(t)
			if tc.nilTypesystem {
				resolved = nil
			}
			cmd := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved))
			req := browserWriteRequest(tuple.NewTupleKey("document:one", "viewer", "user:anne"))
			req.StoreId, req.AuthorizationModelId = tc.store, tc.model
			resp, err := cmd.Execute(context.Background(), req)
			require.Equal(t, tc.wantReads, ds.reads.Load())
			if tc.wantError {
				require.Error(t, err)
				require.Nil(t, resp)
				require.Zero(t, ds.writes.Load())
				return
			}
			require.NoError(t, err)
			require.NotNil(t, resp)
			require.EqualValues(t, 1, ds.writes.Load())
			stored, err := ds.ReadUserTuple(context.Background(), tc.store, req.Writes.TupleKeys[0], storage.ReadUserTupleOptions{})
			require.NoError(t, err)
			require.True(t, proto.Equal(req.Writes.TupleKeys[0], stored.GetKey()))
		})
	}
}

func TestBrowserWriteResolvedTypesystemValidationParity(t *testing.T) {
	for _, tc := range []struct {
		name string
		edit func(*openfgav1.WriteRequest)
	}{
		{"wrong_user_type", func(r *openfgav1.WriteRequest) { r.Writes.TupleKeys[0].User = "group:eng" }},
		{"unknown_relation", func(r *openfgav1.WriteRequest) { r.Writes.TupleKeys[0].Relation = "missing" }},
		{"computed_relation", func(r *openfgav1.WriteRequest) { r.Writes.TupleKeys[0].Relation = "computed" }},
		{"unknown_condition", func(r *openfgav1.WriteRequest) {
			r.Writes.TupleKeys[0].Condition = &openfgav1.RelationshipCondition{Name: "missing"}
		}},
		{"duplicate_tuple", func(r *openfgav1.WriteRequest) {
			r.Writes.TupleKeys = append(r.Writes.TupleKeys, proto.Clone(r.Writes.TupleKeys[0]).(*openfgav1.TupleKey))
		}},
		{"tuple_limit", func(r *openfgav1.WriteRequest) {
			r.Writes.TupleKeys = append(r.Writes.TupleKeys, tuple.NewTupleKey("document:two", "viewer", "user:anne"), tuple.NewTupleKey("document:three", "viewer", "user:anne"))
		}},
		{"invalid_delete", func(r *openfgav1.WriteRequest) {
			r.Deletes = &openfgav1.WriteRequestDeletes{TupleKeys: []*openfgav1.TupleKeyWithoutCondition{{Object: "document:two", Relation: "viewer", User: ""}}}
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var originalError error
			for _, reuse := range []bool{false, true} {
				ds, resolved := newBrowserWriteFixture(t)
				if !reuse {
					resolved = nil
				}
				req := browserWriteRequest(tuple.NewTupleKey("document:one", "viewer", "user:anne"))
				tc.edit(req)
				resp, err := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved)).Execute(context.Background(), req)
				require.Error(t, err)
				require.Nil(t, resp)
				require.Zero(t, ds.writes.Load())
				if !reuse {
					originalError = err
				} else {
					require.Equal(t, status.Convert(originalError).Proto(), status.Convert(err).Proto())
					require.Zero(t, ds.reads.Load())
				}
			}
		})
	}
}

func TestBrowserWriteResolvedTypesystemRejectsInvalidSchema(t *testing.T) {
	var originalError error
	for _, reuse := range []bool{false, true} {
		ds, _ := newBrowserWriteFixture(t)
		model, err := ds.OpenFGADatastore.ReadAuthorizationModel(context.Background(), browserWriteStore, browserWriteModel)
		require.NoError(t, err)
		model = proto.Clone(model).(*openfgav1.AuthorizationModel)
		model.SchemaVersion = typesystem.SchemaVersion1_0
		require.NoError(t, ds.WriteAuthorizationModel(context.Background(), browserWriteStore, model))
		// The public option can receive New(), which does not validate schema.
		resolved, err := typesystem.New(model)
		require.NoError(t, err)
		if !reuse {
			resolved = nil
		}
		resp, err := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved)).Execute(context.Background(),
			browserWriteRequest(tuple.NewTupleKey("document:one", "viewer", "user:anne")))
		require.Error(t, err)
		require.Nil(t, resp)
		require.Zero(t, ds.writes.Load())
		if !reuse {
			originalError = err
			require.EqualValues(t, 1, ds.reads.Load())
		} else {
			require.Equal(t, status.Convert(originalError).Proto(), status.Convert(err).Proto())
			require.Zero(t, ds.reads.Load())
		}
	}
}

func TestBrowserWriteResolvedTypesystemConditions(t *testing.T) {
	for _, tc := range []struct {
		name      string
		condition *openfgav1.RelationshipCondition
		byteLimit int
		wantError bool
	}{
		{name: "missing_condition", wantError: true},
		{name: "invalid_parameter_type", condition: &openfgav1.RelationshipCondition{Name: "small", Context: testutils.MustNewStruct(t, map[string]any{"count": "bad"})}, wantError: true},
		{name: "condition_context_limit", condition: &openfgav1.RelationshipCondition{Name: "small", Context: testutils.MustNewStruct(t, map[string]any{"count": 1})}, byteLimit: 1, wantError: true},
		// Write validates condition types; it must not evaluate a Check or reject
		// a well-typed tuple just because its condition currently evaluates false.
		{name: "valid_condition_false", condition: &openfgav1.RelationshipCondition{Name: "small", Context: testutils.MustNewStruct(t, map[string]any{"count": 100})}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var originalError error
			for _, reuse := range []bool{false, true} {
				backend := memory.New()
				t.Cleanup(backend.Close)
				model := parser.MustTransformDSLToProto(`
model
  schema 1.1
type user
type document
  relations
    define viewer: [user with small]
condition small(count: int) { count < 10 }
`)
				model.Id = browserWriteModel
				resolved, err := typesystem.NewAndValidate(context.Background(), model)
				require.NoError(t, err)
				require.NoError(t, backend.WriteAuthorizationModel(context.Background(), browserWriteStore, model))
				ds := &browserWriteDatastore{OpenFGADatastore: backend}
				if !reuse {
					resolved = nil
				}
				opts := []WriteCommandOption{WithWriteCmdResolvedTypesystem(browserWriteStore, resolved)}
				if tc.byteLimit != 0 {
					opts = append(opts, WithConditionContextByteLimit(tc.byteLimit))
				}
				key := tuple.NewTupleKey("document:one", "viewer", "user:anne")
				key.Condition = tc.condition
				resp, err := NewWriteCommand(ds, opts...).Execute(context.Background(), browserWriteRequest(key))
				if tc.wantError {
					require.Error(t, err)
					require.Nil(t, resp)
					require.Zero(t, ds.writes.Load())
					if !reuse {
						originalError = err
					} else {
						require.Equal(t, status.Convert(originalError).Proto(), status.Convert(err).Proto())
					}
				} else {
					require.NoError(t, err)
					require.NotNil(t, resp)
					stored, err := ds.ReadUserTuple(context.Background(), browserWriteStore, key, storage.ReadUserTupleOptions{})
					require.NoError(t, err)
					require.True(t, proto.Equal(key, stored.GetKey()))
				}
				if reuse {
					require.Zero(t, ds.reads.Load())
				}
			}
		})
	}
}

func TestBrowserWriteResolvedTypesystemRealWritesAndErrors(t *testing.T) {
	ds, resolved := newBrowserWriteFixture(t)
	cmd := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved))
	first := tuple.NewTupleKey("document:one", "viewer", "user:anne")
	second := tuple.NewTupleKey("document:two", "viewer", "user:anne")
	_, err := cmd.Execute(context.Background(), browserWriteRequest(first))
	require.NoError(t, err)
	resp, err := cmd.Execute(context.Background(), browserWriteRequest(first))
	require.Error(t, err, "a real duplicate datastore write must not be acknowledged")
	require.Nil(t, resp)
	require.EqualValues(t, 2, ds.writes.Load())
	req := browserWriteRequest(second)
	req.Deletes = &openfgav1.WriteRequestDeletes{TupleKeys: []*openfgav1.TupleKeyWithoutCondition{
		{Object: first.GetObject(), Relation: first.GetRelation(), User: first.GetUser()},
	}}
	_, err = cmd.Execute(context.Background(), req)
	require.NoError(t, err)
	_, err = ds.ReadUserTuple(context.Background(), browserWriteStore, first, storage.ReadUserTupleOptions{})
	require.ErrorIs(t, err, storage.ErrNotFound)
	_, err = ds.ReadUserTuple(context.Background(), browserWriteStore, second, storage.ReadUserTupleOptions{})
	require.NoError(t, err)
	ds.writeError = errors.New("datastore unavailable")
	third := tuple.NewTupleKey("document:three", "viewer", "user:anne")
	resp, err = cmd.Execute(context.Background(), browserWriteRequest(third))
	require.Error(t, err)
	require.Nil(t, resp)
	_, err = ds.ReadUserTuple(context.Background(), browserWriteStore, third, storage.ReadUserTupleOptions{})
	require.ErrorIs(t, err, storage.ErrNotFound)
	require.Zero(t, ds.reads.Load())
	require.EqualValues(t, 4, ds.writes.Load())
}

func TestBrowserWriteResolvedTypesystemCancellation(t *testing.T) {
	for _, expired := range []bool{false, true} {
		t.Run(fmt.Sprintf("deadline_%t", expired), func(t *testing.T) {
			ds, resolved := newBrowserWriteFixture(t)
			ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(time.Minute))
			defer cancel()
			want := serverErrors.ErrRequestCancelled
			if expired {
				cancel()
				ctx, cancel = context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
				defer cancel()
				want = serverErrors.ErrRequestDeadlineExceeded
			} else {
				cancel()
			}
			ds.inspectWrite = func(actual context.Context) {
				require.Same(t, ctx, actual, "the original request context must reach the datastore")
			}
			key := tuple.NewTupleKey("document:one", "viewer", "user:anne")
			resp, err := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved)).Execute(ctx, browserWriteRequest(key))
			require.Nil(t, resp)
			require.Equal(t, status.Convert(want).Proto(), status.Convert(err).Proto())
			_, err = ds.ReadUserTuple(context.Background(), browserWriteStore, key, storage.ReadUserTupleOptions{})
			require.ErrorIs(t, err, storage.ErrNotFound)
		})
	}
}

func TestBrowserWriteResolvedTypesystemConcurrent(t *testing.T) {
	ds, resolved := newBrowserWriteFixture(t)
	cmd := NewWriteCommand(ds, WithWriteCmdResolvedTypesystem(browserWriteStore, resolved))
	results := make(chan error, 8)
	for i := 0; i < cap(results); i++ {
		go func(i int) {
			_, err := cmd.Execute(context.Background(), browserWriteRequest(tuple.NewTupleKey(fmt.Sprintf("document:%d", i), "viewer", "user:anne")))
			results <- err
		}(i)
	}
	for i := 0; i < cap(results); i++ {
		require.NoError(t, <-results)
	}
	require.Zero(t, ds.reads.Load())
	require.EqualValues(t, cap(results), ds.writes.Load())
}
