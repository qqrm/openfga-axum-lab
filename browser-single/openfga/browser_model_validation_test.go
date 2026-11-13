package typesystem

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"os"
	"sort"
	"sync"
	"testing"
	"time"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
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
	installBrowserValidation(t, prepareBrowserValidation(t, model))
	want, err := newAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	got, err := NewAndValidate(context.Background(), proto.Clone(model).(*openfgav1.AuthorizationModel))
	if err != nil {
		t.Fatal(err)
	}
	if got.GetSchemaVersion() != want.GetSchemaVersion() || len(got.GetAllRelations()) != len(want.GetAllRelations()) {
		t.Fatal("schema metadata changed")
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
