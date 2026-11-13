package typesystem

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"os"
	"sort"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"gonum.org/v1/gonum/graph"
	"gonum.org/v1/gonum/graph/traverse"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	"github.com/openfga/openfga/pkg/storage"
	"github.com/openfga/openfga/pkg/storage/memory"
)

func browserValidationFixture(t *testing.T) *openfgav1.AuthorizationModel {
	t.Helper()
	path := os.Getenv("BROWSER_OPENFGA_TEST_MODEL")
	if path == "" {
		t.Fatal("BROWSER_OPENFGA_TEST_MODEL must name the pinned fga/model.json")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(data) != 52709 || fmt.Sprintf("%x", sha256.Sum256(data)) != "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d" {
		t.Fatal("test model is not the exact pinned fixture")
	}
	model := &openfgav1.AuthorizationModel{}
	if err := protojson.Unmarshal(data, model); err != nil {
		t.Fatal(err)
	}
	model.Id = "01J00000000000000000000000"
	return model
}

func isolateBrowserValidation(t *testing.T) {
	t.Helper()
	previous := browserModelValidation.Swap(nil)
	t.Cleanup(func() { browserModelValidation.Store(previous) })
}

func prepareBrowserValidation(t *testing.T, model *openfgav1.AuthorizationModel) *BrowserModelValidation {
	t.Helper()
	prepared, err := PrepareBrowserModelValidation(context.Background(), model)
	if err != nil {
		t.Fatal(err)
	}
	return prepared
}

func installBrowserValidation(t *testing.T, prepared *BrowserModelValidation) {
	t.Helper()
	if err := InstallBrowserModelValidation(context.Background(), prepared); err != nil {
		t.Fatal(err)
	}
}

func firstBrowserRelation(t *testing.T, model *openfgav1.AuthorizationModel) (*openfgav1.TypeDefinition, string) {
	t.Helper()
	for _, td := range model.GetTypeDefinitions() {
		if len(td.GetRelations()) == 0 {
			continue
		}
		names := make([]string, 0, len(td.GetRelations()))
		for name := range td.GetRelations() {
			names = append(names, name)
		}
		sort.Strings(names)
		return td, names[0]
	}
	t.Fatal("fixture has no relations")
	return nil, ""
}

func TestBrowserModelDescriptorCoverage(t *testing.T) {
	// matches constructs a read-only body view. A new upstream field must not
	// silently disappear from that view, even if it is absent in this fixture.
	want := map[protoreflect.FieldNumber]protoreflect.Name{
		1: "id", 2: "schema_version", 3: "type_definitions", 4: "conditions",
	}
	fields := (&openfgav1.AuthorizationModel{}).ProtoReflect().Descriptor().Fields()
	if fields.Len() != len(want) {
		t.Fatalf("AuthorizationModel fields changed: got %d, want %d", fields.Len(), len(want))
	}
	for i := 0; i < fields.Len(); i++ {
		field := fields.Get(i)
		if want[field.Number()] != field.Name() {
			t.Fatalf("unreviewed AuthorizationModel field: %d %s", field.Number(), field.Name())
		}
	}
}

func TestBrowserModelExactBodyAndIdentity(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	before := proto.Clone(model)
	prepared := prepareBrowserValidation(t, model)
	if browserModelValidation.Load() != nil {
		t.Fatal("preparation published the cache")
	}
	installBrowserValidation(t, prepared)
	model.Id = "01J00000000000000000000001"
	if !prepared.matches(model) {
		t.Fatal("generated ID incorrectly participates in the schema key")
	}
	if err := ValidateBrowserAuthorizationModel(context.Background(), model); err != nil {
		t.Fatal(err)
	}
	typesys, err := NewAndValidate(context.Background(), model)
	if err != nil || typesys.GetAuthorizationModelID() != model.Id {
		t.Fatalf("real model identity was lost: typesystem=%v error=%v", typesys, err)
	}
	before.(*openfgav1.AuthorizationModel).Id = model.Id
	if !proto.Equal(before, model) {
		t.Fatal("comparison or validation mutated the incoming model")
	}
}

func TestBrowserModelKeyMismatches(t *testing.T) {
	isolateBrowserValidation(t)
	original := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, original)
	installBrowserValidation(t, prepared)
	initial := prepared.resolverTypesystem.Load()
	for _, test := range []struct {
		name   string
		change func(*openfgav1.AuthorizationModel)
	}{
		{"schema", func(m *openfgav1.AuthorizationModel) { m.SchemaVersion = "9.9" }},
		{"nested_rewrite", func(m *openfgav1.AuthorizationModel) {
			td, relation := firstBrowserRelation(t, m)
			td.Relations[relation] = ComputedUserset("missing_browser_relation")
		}},
		{"metadata", func(m *openfgav1.AuthorizationModel) {
			td, _ := firstBrowserRelation(t, m)
			td.Metadata = &openfgav1.Metadata{}
		}},
		{"conditions", func(m *openfgav1.AuthorizationModel) {
			m.Conditions = map[string]*openfgav1.Condition{"browser": {Name: "browser", Expression: "true"}}
		}},
		{"unknown_root", func(m *openfgav1.AuthorizationModel) { m.ProtoReflect().SetUnknown([]byte{0xa0, 0x06, 0x01}) }},
		{"unknown_nested", func(m *openfgav1.AuthorizationModel) {
			m.TypeDefinitions[0].ProtoReflect().SetUnknown([]byte{0xa0, 0x06, 0x01})
		}},
		{"type_order", func(m *openfgav1.AuthorizationModel) {
			m.TypeDefinitions[0], m.TypeDefinitions[1] = m.TypeDefinitions[1], m.TypeDefinitions[0]
		}},
	} {
		t.Run(test.name, func(t *testing.T) {
			model := proto.Clone(original).(*openfgav1.AuthorizationModel)
			test.change(model)
			if prepared.matches(model) {
				t.Fatal("different schema body matched the validated entry")
			}
			_, want := newAndValidate(context.Background(), model)
			_, got := NewAndValidate(context.Background(), model)
			errOnly := ValidateBrowserAuthorizationModel(context.Background(), model)
			if fmt.Sprint(got) != fmt.Sprint(want) || fmt.Sprint(errOnly) != fmt.Sprint(want) {
				t.Fatalf("fallback differs: raw=%v wrapper=%v error-only=%v", want, got, errOnly)
			}
			datastore, _, resolve := browserMemoryResolver(t)
			storeID := "01J00000000000000000000100"
			persistBrowserResolverModel(t, datastore, storeID, model)
			resolved, resolverErr := resolve(context.Background(), storeID, model.GetId())
			if want == nil {
				if resolverErr != nil || resolved == nil || resolved == initial {
					t.Fatalf("valid mismatching body did not use the ordinary resolver: %v", resolverErr)
				}
			} else if fmt.Sprint(resolverErr) != fmt.Sprintf("%v: %v", ErrInvalidModel, want) {
				t.Fatalf("invalid persisted model changed its error: %v", resolverErr)
			}
			if prepared.resolverTypesystem.Load() != initial {
				t.Fatal("mismatching persisted model consumed the startup object")
			}
		})
	}
	if prepared.matches(nil) {
		t.Fatal("nil model matched the entry")
	}
}

func TestBrowserModelInvalidFallback(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	// Neither API may treat an absent entry as proof of validity.
	model.SchemaVersion = "9.9"
	for _, installed := range []bool{false, true} {
		if installed {
			installBrowserValidation(t, prepareBrowserValidation(t, browserValidationFixture(t)))
		}
		if _, err := NewAndValidate(context.Background(), model); !errors.Is(err, ErrInvalidSchemaVersion) {
			t.Fatalf("invalid schema accepted (cache installed=%t): %v", installed, err)
		}
		if err := ValidateBrowserAuthorizationModel(context.Background(), model); !errors.Is(err, ErrInvalidSchemaVersion) {
			t.Fatalf("error-only path accepted invalid schema: %v", err)
		}
	}
	conditional := browserValidationFixture(t)
	conditional.Conditions = map[string]*openfgav1.Condition{"broken": {Name: "broken", Expression: "("}}
	if _, err := NewAndValidate(context.Background(), conditional); err == nil {
		t.Fatal("conditional model skipped full CEL validation")
	}
	if err := ValidateBrowserAuthorizationModel(context.Background(), conditional); err == nil {
		t.Fatal("error-only path skipped full CEL validation")
	}
}

func TestBrowserModelPreparationAndInstall(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	invalid := proto.Clone(model).(*openfgav1.AuthorizationModel)
	invalid.SchemaVersion = "9.9"
	conditional := proto.Clone(model).(*openfgav1.AuthorizationModel)
	conditional.Conditions = map[string]*openfgav1.Condition{"valid": {Name: "valid", Expression: "true"}}
	for _, input := range []*openfgav1.AuthorizationModel{nil, invalid, conditional} {
		if prepared, err := PrepareBrowserModelValidation(context.Background(), input); err == nil || prepared != nil {
			t.Fatalf("unacceptable model was prepared: prepared=%v error=%v", prepared, err)
		}
		if browserModelValidation.Load() != nil {
			t.Fatal("failed preparation published a cache entry")
		}
	}
	for _, unprepared := range []*BrowserModelValidation{nil, {}, {body: model}, {validated: true}} {
		if err := InstallBrowserModelValidation(context.Background(), unprepared); err == nil {
			t.Fatal("unprepared/zero result was installed")
		}
	}
	prepared := prepareBrowserValidation(t, model)
	installBrowserValidation(t, prepared)
	installBrowserValidation(t, prepareBrowserValidation(t, proto.Clone(model).(*openfgav1.AuthorizationModel)))
	if browserModelValidation.Load() != prepared {
		t.Fatal("same-body installation replaced the original entry")
	}
	other := proto.Clone(model).(*openfgav1.AuthorizationModel)
	other.SchemaVersion = "1.1"
	if err := InstallBrowserModelValidation(context.Background(), prepareBrowserValidation(t, other)); err == nil {
		t.Fatal("different body overwrote the installed entry")
	}
	if browserModelValidation.Load() != prepared {
		t.Fatal("failed installation changed the original entry")
	}
}

// Trigger real cancellation at a selected boundary without timing-sensitive
// sleeps or replacing any production validator/constructor with a stub.
type browserCancelOnCheck struct {
	context.Context
	cancel context.CancelFunc
	when   func() bool
}

func (c *browserCancelOnCheck) Err() error {
	if c.when() {
		c.cancel()
	}
	return c.Context.Err()
}

func TestBrowserModelCancellation(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	canceled, cancel := context.WithCancel(context.Background())
	cancel()
	expired, expire := context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
	defer expire()
	for _, ctx := range []context.Context{canceled, expired} {
		if got, err := PrepareBrowserModelValidation(ctx, model); !errors.Is(err, ctx.Err()) || got != nil {
			t.Fatalf("canceled preparation succeeded: %v", err)
		}
		if err := InstallBrowserModelValidation(ctx, prepared); !errors.Is(err, ctx.Err()) {
			t.Fatalf("canceled installation succeeded: %v", err)
		}
		if browserModelValidation.Load() != nil {
			t.Fatal("cancellation published an entry")
		}
	}
	installBrowserValidation(t, prepared)
	for _, ctx := range []context.Context{canceled, expired} {
		if got, err := NewAndValidate(ctx, model); !errors.Is(err, ctx.Err()) || got != nil {
			t.Fatalf("canceled construction succeeded: %v", err)
		}
		if err := ValidateBrowserAuthorizationModel(ctx, model); !errors.Is(err, ctx.Err()) {
			t.Fatalf("canceled error-only validation succeeded: %v", err)
		}
		if err := InstallBrowserModelValidation(ctx, prepared); !errors.Is(err, ctx.Err()) || browserModelValidation.Load() != prepared {
			t.Fatalf("canceled no-op removed an existing entry: %v", err)
		}
	}
	for _, operation := range []struct {
		name     string
		cancelAt int
		invoke   func(context.Context) error
	}{
		{"prepare_after_full_validation", 3, func(ctx context.Context) error {
			got, err := PrepareBrowserModelValidation(ctx, model)
			if got != nil {
				t.Error("late preparation returned a reusable proof")
			}
			return err
		}},
		{"fresh_constructor", 2, func(ctx context.Context) error {
			got, err := NewAndValidate(ctx, model)
			if got != nil {
				t.Error("late construction returned a TypeSystem")
			}
			return err
		}},
		{"error_only_hit", 2, func(ctx context.Context) error { return ValidateBrowserAuthorizationModel(ctx, model) }},
	} {
		t.Run(operation.name, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			checks := 0
			late := &browserCancelOnCheck{Context: ctx, cancel: cancel, when: func() bool {
				checks++
				return checks >= operation.cancelAt
			}}
			if err := operation.invoke(late); !errors.Is(err, context.Canceled) {
				t.Fatalf("late success won over cancellation: %v", err)
			}
		})
	}
}

func TestBrowserModelInstallCancellationRollsBack(t *testing.T) {
	isolateBrowserValidation(t)
	prepared := prepareBrowserValidation(t, browserValidationFixture(t))
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	late := &browserCancelOnCheck{Context: ctx, cancel: cancel, when: func() bool {
		return browserModelValidation.Load() == prepared
	}}
	if err := InstallBrowserModelValidation(late, prepared); !errors.Is(err, context.Canceled) {
		t.Fatalf("publication ignored cancellation: %v", err)
	}
	if browserModelValidation.Load() != nil {
		t.Fatal("late publication remained installed")
	}
}

func TestBrowserModelOwnsOnlyPrivateValidationBody(t *testing.T) {
	isolateBrowserValidation(t)
	input := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, input)
	input.SchemaVersion = "changed_after_prepare"
	td, relation := firstBrowserRelation(t, input)
	td.Relations[relation] = ComputedUserset("changed_after_prepare")
	installBrowserValidation(t, prepared)
	model := browserValidationFixture(t)
	if !prepared.matches(model) {
		t.Fatal("input mutation poisoned the private validation key")
	}
	a, err := NewAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	b, err := NewAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	if a == b || a.authorizationModelGraph == b.authorizationModelGraph ||
		(a.authzWeightedGraph != nil && a.authzWeightedGraph == b.authzWeightedGraph) {
		t.Fatal("fresh TypeSystems shared a graph or wrapper")
	}
	td, relation = firstBrowserRelation(t, model)
	a.relations[td.Type][relation].Rewrite.Userset = ComputedUserset("changed_result").Userset
	a.computedRelations.Store("browser-isolation", "changed_result")
	if _, ok := b.computedRelations.Load("browser-isolation"); ok {
		t.Fatal("computed-relation cache is shared")
	}
	if !proto.Equal(b.relations[td.Type][relation].GetRewrite(), td.Relations[relation]) || !prepared.matches(model) {
		t.Fatal("result mutation changed another TypeSystem or the validation key")
	}
}

func TestBrowserModelGraphBehaviorMatchesRawValidator(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	installBrowserValidation(t, prepared)
	model.Id = "01J00000000000000000000200"
	want, err := newAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	got, err := NewAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	assertBrowserGraphEquivalent(t, model, want, got)
	datastore, _, resolve := browserMemoryResolver(t)
	storeID := "01J00000000000000000000100"
	persistBrowserResolverModel(t, datastore, storeID, model)
	initial := prepared.resolverTypesystem.Load()
	transferred, err := resolve(context.Background(), storeID, model.GetId())
	if err != nil || transferred != initial || transferred == nil {
		t.Fatalf("the persisted model did not receive the startup TypeSystem: %v", err)
	}
	assertBrowserGraphEquivalent(t, model, want, transferred)
}

func assertBrowserGraphEquivalent(t *testing.T, model *openfgav1.AuthorizationModel, want, got *TypeSystem) {
	t.Helper()
	if got.GetSchemaVersion() != want.GetSchemaVersion() || len(got.GetAllRelations()) != len(want.GetAllRelations()) ||
		got.GetAuthorizationModelID() != want.GetAuthorizationModelID() {
		t.Fatal("schema metadata or actual model identity changed")
	}
	// Compare graph behavior rather than private graph identities: upstream
	// creates random ULIDs for operator nodes on every construction.
	for objectType, relations := range want.GetAllRelations() {
		for relation, wantRelation := range relations {
			gotRelation, err := got.GetRelation(objectType, relation)
			if err != nil || !proto.Equal(gotRelation, wantRelation) {
				t.Fatalf("relation differs: %s#%s: %v", objectType, relation, err)
			}
			for _, source := range model.GetTypeDefinitions() {
				user := source.Type + ":browser"
				wantPath, wantErr := want.PathExists(user, relation, objectType)
				gotPath, gotErr := got.PathExists(user, relation, objectType)
				if gotPath != wantPath || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
					t.Fatalf("path differs for %s -> %s#%s", source.Type, objectType, relation)
				}
				if got.UsersetUseRecursiveResolver(objectType, relation, source.Type) != want.UsersetUseRecursiveResolver(objectType, relation, source.Type) {
					t.Fatalf("recursive strategy differs for %s#%s", objectType, relation)
				}
				for _, ttu := range flattenUserset(wantRelation.GetRewrite()) {
					if got.TTUUseWeight2Resolver(objectType, relation, source.Type, ttu) != want.TTUUseWeight2Resolver(objectType, relation, source.Type, ttu) ||
						got.TTUUseRecursiveResolver(objectType, relation, source.Type, ttu) != want.TTUUseRecursiveResolver(objectType, relation, source.Type, ttu) {
						t.Fatalf("TTU strategy differs for %s#%s", objectType, relation)
					}
				}
			}
		}
	}
}

func TestBrowserModelConcurrentRequests(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	const workers = 8
	start := make(chan struct{})
	errorsFound := make(chan error, workers)
	var done sync.WaitGroup
	for i := 0; i < workers; i++ {
		input := proto.Clone(model).(*openfgav1.AuthorizationModel)
		input.Id = fmt.Sprintf("%026d", i+1)
		done.Add(1)
		go func() {
			defer done.Done()
			<-start
			if err := InstallBrowserModelValidation(context.Background(), prepared); err != nil {
				errorsFound <- err
				return
			}
			if err := ValidateBrowserAuthorizationModel(context.Background(), input); err != nil {
				errorsFound <- err
				return
			}
			typesys, err := NewAndValidate(context.Background(), input)
			if err != nil || typesys.GetAuthorizationModelID() != input.Id {
				errorsFound <- fmt.Errorf("concurrent request lost its identity: %v", err)
				return
			}
			typesys.computedRelations.Store("browser-concurrent", input.Id)
		}()
	}
	close(start)
	done.Wait()
	close(errorsFound)
	for err := range errorsFound {
		t.Error(err)
	}
	if browserModelValidation.Load() != prepared || !prepared.matches(model) {
		t.Fatal("concurrent calls changed the validation entry")
	}
}

type browserModelReadCounter struct {
	storage.AuthorizationModelReadBackend
	reads  atomic.Int32
	latest atomic.Int32
}

func (r *browserModelReadCounter) ReadAuthorizationModel(ctx context.Context, storeID, modelID string) (*openfgav1.AuthorizationModel, error) {
	r.reads.Add(1)
	return r.AuthorizationModelReadBackend.ReadAuthorizationModel(ctx, storeID, modelID)
}

func (r *browserModelReadCounter) FindLatestAuthorizationModel(ctx context.Context, storeID string) (*openfgav1.AuthorizationModel, error) {
	r.latest.Add(1)
	return r.AuthorizationModelReadBackend.FindLatestAuthorizationModel(ctx, storeID)
}

func browserMemoryResolver(t *testing.T) (storage.OpenFGADatastore, *browserModelReadCounter, TypesystemResolverFunc) {
	t.Helper()
	datastore := memory.New()
	t.Cleanup(datastore.Close)
	reader := &browserModelReadCounter{AuthorizationModelReadBackend: datastore}
	resolve, stop, err := MemoizedTypesystemResolverFunc(reader)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(stop)
	return datastore, reader, resolve
}

func persistBrowserResolverModel(t *testing.T, datastore storage.OpenFGADatastore, storeID string, model *openfgav1.AuthorizationModel) {
	t.Helper()
	if _, err := datastore.CreateStore(context.Background(), &openfgav1.Store{Id: storeID, Name: "browser-transfer"}); err != nil {
		t.Fatal(err)
	}
	if err := datastore.WriteAuthorizationModel(context.Background(), storeID, model); err != nil {
		t.Fatal(err)
	}
}

func TestBrowserModelResolverRequiresPersistedModel(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	installBrowserValidation(t, prepared)
	initial := prepared.resolverTypesystem.Load()
	if initial == nil {
		t.Fatal("successful preparation discarded the validated TypeSystem")
	}
	model.Id = "01J00000000000000000000200"
	if err := ValidateBrowserAuthorizationModel(context.Background(), model); err != nil {
		t.Fatal(err)
	}
	ordinary, err := NewAndValidate(context.Background(), model)
	if err != nil || ordinary == initial || prepared.resolverTypesystem.Load() != initial {
		t.Fatalf("a non-resolver caller consumed the startup object: %v", err)
	}
	datastore, reader, resolve := browserMemoryResolver(t)
	storeID := "01J00000000000000000000100"
	if got, err := resolve(context.Background(), storeID, model.GetId()); got != nil || !errors.Is(err, ErrModelNotFound) {
		t.Fatalf("an unpersisted model was resolved: %v", err)
	}
	if reader.reads.Load() != 1 || prepared.resolverTypesystem.Load() != initial {
		t.Fatal("missing-model lookup was bypassed or consumed the startup object")
	}
	persistBrowserResolverModel(t, datastore, storeID, model)
	if got, err := resolve(context.Background(), "01J00000000000000000000101", model.GetId()); got != nil || !errors.Is(err, ErrModelNotFound) {
		t.Fatalf("a model from another store was resolved: %v", err)
	}
	if prepared.resolverTypesystem.Load() != initial {
		t.Fatal("a wrong-store request consumed the startup object")
	}
	got, err := resolve(context.Background(), storeID, model.GetId())
	if err != nil || got != initial || got.GetAuthorizationModelID() != model.GetId() {
		t.Fatalf("persisted model transfer failed: %v", err)
	}
	if reader.reads.Load() != 3 || prepared.resolverTypesystem.Load() != nil {
		t.Fatal("transfer bypassed its actual read or retained a second owner")
	}
	again, err := resolve(context.Background(), storeID, model.GetId())
	if err != nil || again != got || reader.reads.Load() != 3 {
		t.Fatalf("the existing store/model cache changed: %v", err)
	}
}

func TestBrowserModelResolverLatestKeepsActualIdentity(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	installBrowserValidation(t, prepared)
	initial := prepared.resolverTypesystem.Load()
	model.Id = "01J00000000000000000000200"
	datastore, reader, resolve := browserMemoryResolver(t)
	storeID := "01J00000000000000000000100"
	persistBrowserResolverModel(t, datastore, storeID, model)
	got, err := resolve(context.Background(), storeID, "")
	if err != nil || got != initial || got.GetAuthorizationModelID() != model.GetId() {
		t.Fatalf("latest-model transfer lost its actual identity: %v", err)
	}
	if reader.latest.Load() != 1 || reader.reads.Load() != 0 {
		t.Fatal("the real latest-model lookup changed")
	}
}

func TestBrowserModelResolverPreconditionsDoNotClaim(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	installBrowserValidation(t, prepared)
	initial := prepared.resolverTypesystem.Load()
	for _, input := range []struct {
		storeID string
		modelID string
		model   *openfgav1.AuthorizationModel
	}{
		{"", model.GetId(), model},
		{"store", "", model},
		{"store", "01J00000000000000000000200", model},
		{"store", model.GetId(), nil},
	} {
		_, want := NewAndValidate(context.Background(), input.model)
		got, err := newAndValidateResolvedModel(context.Background(), input.storeID, input.modelID, input.model)
		if fmt.Sprint(err) != fmt.Sprint(want) || got == initial {
			t.Fatalf("invalid transfer precondition changed the ordinary path: %v", err)
		}
		if prepared.resolverTypesystem.Load() != initial {
			t.Fatal("invalid transfer precondition consumed the startup object")
		}
	}
}

func TestBrowserModelResolverCancellation(t *testing.T) {
	for _, afterClaim := range []bool{false, true} {
		t.Run(fmt.Sprintf("after_claim_%t", afterClaim), func(t *testing.T) {
			isolateBrowserValidation(t)
			model := browserValidationFixture(t)
			prepared := prepareBrowserValidation(t, model)
			installBrowserValidation(t, prepared)
			initial := prepared.resolverTypesystem.Load()
			model.Id = "01J00000000000000000000200"
			datastore, reader, resolve := browserMemoryResolver(t)
			storeID := "01J00000000000000000000100"
			persistBrowserResolverModel(t, datastore, storeID, model)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			canceled := &browserCancelOnCheck{Context: ctx, cancel: cancel, when: func() bool {
				if afterClaim {
					return prepared.resolverTypesystem.Load() == nil
				}
				return reader.reads.Load() > 0
			}}
			got, err := resolve(canceled, storeID, model.GetId())
			if got != nil || fmt.Sprint(err) != fmt.Sprintf("%v: %v", ErrInvalidModel, context.Canceled) {
				t.Fatalf("canceled resolver returned a TypeSystem or changed its error: %v", err)
			}
			if afterClaim && prepared.resolverTypesystem.Load() != nil {
				t.Fatal("canceled claim was returned to the slot")
			}
			if !afterClaim && prepared.resolverTypesystem.Load() != initial {
				t.Fatal("pre-claim cancellation consumed the slot")
			}
			fresh, err := resolve(context.Background(), storeID, model.GetId())
			if err != nil || fresh == nil || (afterClaim && fresh == initial) || (!afterClaim && fresh != initial) {
				t.Fatalf("cancellation published or recycled an invalid resolver result: %v", err)
			}
		})
	}
}

func TestBrowserModelResolverTransferKeyIsolation(t *testing.T) {
	isolateBrowserValidation(t)
	input := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, input)
	initial := prepared.resolverTypesystem.Load()
	input.SchemaVersion = "mutated_after_prepare"
	installBrowserValidation(t, prepared)
	model := browserValidationFixture(t)
	model.Id = "01J00000000000000000000200"
	before := proto.Clone(model)
	datastore, _, resolve := browserMemoryResolver(t)
	storeID := "01J00000000000000000000100"
	persistBrowserResolverModel(t, datastore, storeID, model)
	got, err := resolve(context.Background(), storeID, model.GetId())
	if err != nil || got != initial || got.GetSchemaVersion() != model.GetSchemaVersion() {
		t.Fatalf("startup input mutation leaked into the transferred object: %v", err)
	}
	td, relation := firstBrowserRelation(t, model)
	owned, ok := got.GetTypeDefinition(td.GetType())
	if !ok {
		t.Fatal("transferred type definition is missing")
	}
	owned.Relations[relation] = ComputedUserset("mutated_after_transfer")
	got.computedRelations.Store("transfer-key-isolation", "mutated")
	if !prepared.matches(model) || !proto.Equal(model, before) {
		t.Fatal("a transferred getter mutated the immutable comparison key or datastore model")
	}
	if err := ValidateBrowserAuthorizationModel(context.Background(), model); err != nil {
		t.Fatal(err)
	}
	fresh, err := NewAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil || fresh == got {
		t.Fatalf("ordinary construction reused mutable transferred state: %v", err)
	}
	if _, ok := fresh.computedRelations.Load("transfer-key-isolation"); ok {
		t.Fatal("the transferred computed-relations map was shared")
	}
	freshType, ok := fresh.GetTypeDefinition(td.GetType())
	if !ok || !proto.Equal(freshType.GetRelations()[relation], td.GetRelations()[relation]) {
		t.Fatal("the transferred schema was shared with a fresh TypeSystem")
	}
}

func TestBrowserModelResolverConcurrentStoresClaimOnce(t *testing.T) {
	isolateBrowserValidation(t)
	prepared := prepareBrowserValidation(t, browserValidationFixture(t))
	installBrowserValidation(t, prepared)
	initial := prepared.resolverTypesystem.Load()
	datastore, reader, resolve := browserMemoryResolver(t)
	stores := []string{"01J00000000000000000000100", "01J00000000000000000000101"}
	models := []string{"01J00000000000000000000200", "01J00000000000000000000201"}
	for index, storeID := range stores {
		model := browserValidationFixture(t)
		model.Id = models[index]
		persistBrowserResolverModel(t, datastore, storeID, model)
	}
	type outcome struct {
		index   int
		typesys *TypeSystem
		err     error
	}
	start := make(chan struct{})
	results := make(chan outcome, len(stores))
	var done sync.WaitGroup
	for index, storeID := range stores {
		done.Add(1)
		go func(index int, storeID string) {
			defer done.Done()
			<-start
			typesys, err := resolve(context.Background(), storeID, models[index])
			results <- outcome{index: index, typesys: typesys, err: err}
		}(index, storeID)
	}
	close(start)
	done.Wait()
	close(results)
	claimed := 0
	var instances []*TypeSystem
	for result := range results {
		if result.err != nil || result.typesys == nil || result.typesys.GetAuthorizationModelID() != models[result.index] {
			t.Fatalf("concurrent model binding changed: %v", result.err)
		}
		if result.typesys == initial {
			claimed++
		}
		instances = append(instances, result.typesys)
	}
	if claimed != 1 || instances[0] == instances[1] || reader.reads.Load() != 2 {
		t.Fatal("concurrent stores did not retain two real reads and exactly one transferred owner")
	}
	installBrowserValidation(t, prepareBrowserValidation(t, browserValidationFixture(t)))
	if prepared.resolverTypesystem.Load() != nil || browserModelValidation.Load() != prepared {
		t.Fatal("same-body reinstallation replenished or replaced the consumed slot")
	}
}

func TestBrowserModelUserPathsPinnedParity(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	got := prepared.resolverTypesystem.Load()
	if got == nil || len(got.browserUserPaths) != 316 {
		t.Fatalf("pinned startup paths were not prepared: %v", got)
	}
	want, err := newAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	if want.browserUserPaths != nil {
		t.Fatal("the original constructor unexpectedly seeded browser paths")
	}
	before := make(map[browserUserPathKey]bool, len(got.browserUserPaths))
	for key, value := range got.browserUserPaths {
		before[key] = value
	}
	users := []string{"user:alice", "user:other-id", "user:*", "user:alice#assignee", "role:analysts#assignee", "missing:alice"}
	for objectType, relations := range want.GetAllRelations() {
		for relation := range relations {
			for _, user := range users {
				wantPath, wantErr := want.PathExists(user, relation, objectType)
				gotPath, gotErr := got.PathExists(user, relation, objectType)
				if gotPath != wantPath || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
					t.Fatalf("seeded path differs for %s -> %s#%s: %v/%v", user, objectType, relation, gotErr, wantErr)
				}
			}
		}
	}
	for _, target := range []browserUserPathKey{
		{objectType: "missing", relation: "viewer"},
		{objectType: "lakekeeper_table", relation: "missing"},
	} {
		wantPath, wantErr := want.PathExists("user:alice", target.relation, target.objectType)
		gotPath, gotErr := got.PathExists("user:alice", target.relation, target.objectType)
		if wantErr == nil || gotPath != wantPath || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
			t.Fatalf("unknown target did not preserve its original error: %v/%v", gotErr, wantErr)
		}
	}
	if len(got.browserUserPaths) != len(before) {
		t.Fatal("queries inserted new entries into the startup-only map")
	}
	for key, value := range before {
		if actual, ok := got.browserUserPaths[key]; !ok || actual != value {
			t.Fatal("queries changed a prepared structural path")
		}
	}
	installBrowserValidation(t, prepared)
	fresh, err := NewAndValidate(context.Background(), model)
	if err != nil || fresh == nil || fresh.browserUserPaths != nil {
		t.Fatalf("a generic constructor shared the startup map: %v", err)
	}
}

func browserUserPathTestModel(sourceType string, relationCount int) *openfgav1.AuthorizationModel {
	relations := make(map[string]*openfgav1.Userset, relationCount)
	metadata := make(map[string]*openfgav1.RelationMetadata, relationCount)
	for i := 0; i < relationCount; i++ {
		relation := fmt.Sprintf("viewer_%03d", i)
		relations[relation] = This()
		metadata[relation] = &openfgav1.RelationMetadata{
			DirectlyRelatedUserTypes: []*openfgav1.RelationReference{{Type: sourceType}},
		}
	}
	return &openfgav1.AuthorizationModel{
		Id:            "01J00000000000000000000300",
		SchemaVersion: "1.2",

		TypeDefinitions: []*openfgav1.TypeDefinition{
			{Type: sourceType},
			{
				Type:      "document",
				Relations: relations,
				Metadata:  &openfgav1.Metadata{Relations: metadata},
			},
		},
	}
}

func TestBrowserModelUserPathsEligibilityAndBound(t *testing.T) {
	for _, test := range []struct {
		name, sourceType string
		relations        int
		wantSeeded       int
	}{
		{name: "no_user_type", sourceType: "person", relations: 2},
		{name: "exact_limit", sourceType: "user", relations: browserUserPathLimit, wantSeeded: browserUserPathLimit},
		{name: "above_limit", sourceType: "user", relations: browserUserPathLimit + 1},
	} {
		t.Run(test.name, func(t *testing.T) {
			isolateBrowserValidation(t)
			model := browserUserPathTestModel(test.sourceType, test.relations)
			prepared := prepareBrowserValidation(t, model)
			got := prepared.resolverTypesystem.Load()
			if got == nil || len(got.browserUserPaths) != test.wantSeeded {
				t.Fatalf("unexpected seed bound: %v", got)
			}
			if test.wantSeeded == 0 && got.browserUserPaths != nil {
				t.Fatal("an ineligible model should retain the nil-map fallback")
			}
			want, err := newAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
			if err != nil {
				t.Fatal(err)
			}
			for _, user := range []string{test.sourceType + ":alice", "user:alice", "role:reader#member"} {
				wantPath, wantErr := want.PathExists(user, "viewer_000", "document")
				gotPath, gotErr := got.PathExists(user, "viewer_000", "document")
				if gotPath != wantPath || fmt.Sprint(gotErr) != fmt.Sprint(wantErr) {
					t.Fatalf("ineligible/capped path changed: %v/%v", gotErr, wantErr)
				}
			}
		})
	}
}

func TestBrowserModelUserPathsFailureDoesNotPublish(t *testing.T) {
	t.Run("graph_error", func(t *testing.T) {
		model := browserUserPathTestModel("user", 2)
		typesys, err := newAndValidate(context.Background(), model)
		if err != nil {
			t.Fatal(err)
		}
		node, err := typesys.authorizationModelGraph.GetNodeByLabel("document#viewer_001")
		if err != nil {
			t.Fatal(err)
		}
		typesys.authorizationModelGraph.RemoveNode(node.ID())
		_, wantErr := typesys.pathExists("user:browser-warmup", "viewer_001", "document")
		if wantErr == nil {
			t.Fatal("the original predicate did not report the damaged graph")
		}
		err = typesys.prepareBrowserUserPaths(context.Background())
		if fmt.Sprint(err) != fmt.Sprint(wantErr) || typesys.browserUserPaths != nil {
			t.Fatalf("a failed seed published partial paths or hid its error: %v", err)
		}
	})
	t.Run("canceled_seed", func(t *testing.T) {
		isolateBrowserValidation(t)
		model := browserUserPathTestModel("user", 2)
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		checks := 0
		duringSeed := &browserCancelOnCheck{Context: ctx, cancel: cancel, when: func() bool {
			checks++
			// Cancel during preparation before any paths can be published.
			// The dedicated walk test below also cancels inside traversal.
			return checks >= 7
		}}
		prepared, err := PrepareBrowserModelValidation(duringSeed, model)
		if !errors.Is(err, context.Canceled) || prepared != nil || checks < 7 {
			t.Fatalf("canceled preparation returned reusable paths: %v", err)
		}
		if browserModelValidation.Load() != nil {
			t.Fatal("canceled seeding published the startup model")
		}
	})
}

func TestBrowserModelUserPathsConcurrentReadonly(t *testing.T) {
	isolateBrowserValidation(t)
	model := browserValidationFixture(t)
	prepared := prepareBrowserValidation(t, model)
	got := prepared.resolverTypesystem.Load()
	if got == nil || len(got.browserUserPaths) != 316 {
		t.Fatal("missing pinned startup map")
	}
	want, err := newAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	wantPath, wantErr := want.PathExists("user:alice", "can_get_metadata", "lakekeeper_table")
	if wantErr != nil {
		t.Fatal(wantErr)
	}
	var workers sync.WaitGroup
	errorsFound := make(chan error, 16)
	for worker := 0; worker < 16; worker++ {
		workers.Add(1)
		go func(index int) {
			defer workers.Done()
			for request := 0; request < 16; request++ {
				path, err := got.PathExists(fmt.Sprintf("user:reader_%d_%d", index, request), "can_get_metadata", "lakekeeper_table")
				if err != nil || path != wantPath {
					errorsFound <- fmt.Errorf("concurrent path changed: %v", err)
					return
				}
			}
		}(worker)
	}
	workers.Wait()
	close(errorsFound)
	for err := range errorsFound {
		t.Error(err)
	}
	if len(got.browserUserPaths) != 316 {
		t.Fatal("concurrent entity IDs grew the structural map")
	}
}

type browserPathWalkGraph struct {
	traverse.Graph
	fromCalls map[int64]int
	cancel    context.CancelFunc
}

func (g *browserPathWalkGraph) From(id int64) graph.Nodes {
	g.fromCalls[id]++
	if g.cancel != nil {
		g.cancel()
	}
	return g.Graph.From(id)
}

func TestBrowserModelUserPathsTwoWalkParity(t *testing.T) {
	model := browserValidationFixture(t)
	typesys, err := newAndValidate(context.Background(), model)
	if err != nil {
		t.Fatal(err)
	}
	var walks []*traverse.BreadthFirst
	for _, label := range []string{"user", "user:*"} {
		from, err := typesys.authorizationModelGraph.GetNodeByLabel(label)
		if err != nil {
			if label == "user:*" {
				continue
			}
			t.Fatal(err)
		}
		counted := &browserPathWalkGraph{
			Graph:     typesys.authorizationModelGraph.DirectedGraph,
			fromCalls: make(map[int64]int),
		}
		walk, err := walkBrowserUserPaths(context.Background(), counted, from)
		if err != nil {
			t.Fatal(err)
		}
		if !walk.Visited(from) {
			t.Fatal("the starting node must be reachable from itself")
		}
		if len(counted.fromCalls) == 0 || len(counted.fromCalls) > typesys.authorizationModelGraph.Nodes().Len() {
			t.Fatalf("unexpected traversal size: %d", len(counted.fromCalls))
		}
		for node, calls := range counted.fromCalls {
			if calls != 1 {
				t.Fatalf("node %d was expanded %d times in one walk", node, calls)
			}
		}
		walks = append(walks, walk)
	}
	if len(walks) < 1 || len(walks) > 2 {
		t.Fatalf("unexpected traversal count: %d", len(walks))
	}
	pairs := 0
	for objectType, relations := range typesys.relations {
		for relation := range relations {
			target, err := typesys.authorizationModelGraph.GetNodeByLabel(objectType + "#" + relation)
			if err != nil {
				t.Fatal(err)
			}
			got := false
			for _, walk := range walks {
				got = got || walk.Visited(target)
			}
			want, err := typesys.pathExists("user:browser-warmup", relation, objectType)
			if err != nil || got != want {
				t.Fatalf("two-walk parity for %s#%s: got %v, want %v, error %v", objectType, relation, got, want, err)
			}
			pairs++
		}
	}
	if pairs != 316 {
		t.Fatalf("checked %d schema pairs, want 316", pairs)
	}
}

func TestBrowserModelUserPathsLookupFallbacks(t *testing.T) {
	t.Run("missing_source_precedes_target", func(t *testing.T) {
		typesys, err := newAndValidate(context.Background(), browserUserPathTestModel("user", 2))
		if err != nil {
			t.Fatal(err)
		}
		for _, label := range []string{"user", "document#viewer_000"} {
			node, err := typesys.authorizationModelGraph.GetNodeByLabel(label)
			if err != nil {
				t.Fatal(err)
			}
			typesys.authorizationModelGraph.RemoveNode(node.ID())
		}
		_, wantErr := typesys.pathExists("user:browser-warmup", "viewer_000", "document")
		if wantErr == nil {
			t.Fatal("the original predicate must report the missing source")
		}
		err = typesys.prepareBrowserUserPaths(context.Background())
		if fmt.Sprint(err) != fmt.Sprint(wantErr) || typesys.browserUserPaths != nil {
			t.Fatalf("source lookup error changed or published paths: %v", err)
		}
	})
	for _, test := range []struct {
		name     string
		wildcard bool
		remove   bool
		want     bool
	}{
		{name: "absent_wildcard"},
		{name: "reachable_wildcard", wildcard: true, want: true},
		{name: "damaged_wildcard", wildcard: true, remove: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			model := browserUserPathTestModel("user", 2)
			model.TypeDefinitions = append(model.TypeDefinitions, &openfgav1.TypeDefinition{Type: "group"})
			reference := &openfgav1.RelationReference{Type: "group"}
			if test.wildcard {
				reference = WildcardRelationReference("user")
			}
			model.TypeDefinitions[1].Metadata.Relations["viewer_001"].DirectlyRelatedUserTypes = []*openfgav1.RelationReference{reference}
			typesys, err := newAndValidate(context.Background(), model)
			if err != nil {
				t.Fatal(err)
			}
			if test.remove {
				node, err := typesys.authorizationModelGraph.GetNodeByLabel("user:*")
				if err != nil {
					t.Fatal(err)
				}
				typesys.authorizationModelGraph.RemoveNode(node.ID())
			}
			direct, err := typesys.authorizationModelGraph.PathExists("user", "document#viewer_001")
			if err != nil || direct {
				t.Fatalf("fixture must require the wildcard fallback: %v/%v", direct, err)
			}
			want, wantErr := typesys.pathExists("user:browser-warmup", "viewer_001", "document")
			if wantErr != nil || want != test.want {
				t.Fatalf("original wildcard behavior changed: %v/%v", want, wantErr)
			}
			if err := typesys.prepareBrowserUserPaths(context.Background()); err != nil {
				t.Fatal(err)
			}
			key := browserUserPathKey{objectType: "document", relation: "viewer_001"}
			got, exists := typesys.browserUserPaths[key]
			if !exists || got != want {
				t.Fatalf("wildcard result was omitted or changed: %v/%v", got, exists)
			}
		})
	}
	t.Run("zero_pairs", func(t *testing.T) {
		typesys, err := newAndValidate(context.Background(), browserUserPathTestModel("user", 0))
		if err != nil {
			t.Fatal(err)
		}
		// No source or target lookup is needed when there are no schema pairs.
		typesys.authorizationModelGraph = nil
		if err := typesys.prepareBrowserUserPaths(context.Background()); err != nil {
			t.Fatal(err)
		}
		if typesys.browserUserPaths == nil || len(typesys.browserUserPaths) != 0 {
			t.Fatal("zero pairs must publish an empty map without graph access")
		}
	})
}

func TestBrowserModelUserPathsCanceledDuringWalk(t *testing.T) {
	typesys, err := newAndValidate(context.Background(), browserUserPathTestModel("user", 2))
	if err != nil {
		t.Fatal(err)
	}
	from, err := typesys.authorizationModelGraph.GetNodeByLabel("user")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	counted := &browserPathWalkGraph{
		Graph:     typesys.authorizationModelGraph.DirectedGraph,
		fromCalls: make(map[int64]int),
		cancel:    cancel,
	}
	walk, err := walkBrowserUserPaths(ctx, counted, from)
	if len(counted.fromCalls) != 1 || !errors.Is(err, context.Canceled) || walk != nil {
		t.Fatalf("cancellation inside graph expansion exposed a partial walk: expanded=%d, error=%v, walk=%v", len(counted.fromCalls), err, walk)
	}
	if err := typesys.prepareBrowserUserPaths(ctx); !errors.Is(err, context.Canceled) || typesys.browserUserPaths != nil {
		t.Fatalf("canceled preparation published partial paths: %v", err)
	}
}
