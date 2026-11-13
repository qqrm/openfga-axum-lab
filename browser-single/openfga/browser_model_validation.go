package typesystem

import (
	"context"
	"errors"
	"log"
	"sync/atomic"

	openfgav1 "github.com/openfga/api/proto/openfga/v1"
	"google.golang.org/protobuf/proto"
)

// BrowserModelValidation records one successful validation of an immutable,
// conditionless model body. Its comparison copy is independent of every
// TypeSystem. One fully validated, privately owned TypeSystem can be transferred
// once after the resolver reads the matching persisted model. This retains no
// tuple, authorization result, request context or preassigned datastore binding.
type BrowserModelValidation struct {
	body               *openfgav1.AuthorizationModel
	validated          bool
	resolverTypesystem atomic.Pointer[TypeSystem]
}

var browserModelValidation atomic.Pointer[BrowserModelValidation]

// PrepareBrowserModelValidation performs the original full validation on a
// private copy. The browser startup caller verifies the pinned file identity
// before calling this function. Preparation cannot publish the result: a worker
// finishing after its startup deadline must not enable the fast path.
func PrepareBrowserModelValidation(ctx context.Context, model *openfgav1.AuthorizationModel) (*BrowserModelValidation, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if model == nil || len(model.GetConditions()) != 0 {
		return nil, errors.New("browser model validation requires a non-nil conditionless model")
	}
	body := proto.Clone(model).(*openfgav1.AuthorizationModel)
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	typesys, err := newAndValidate(ctx, body)
	if contextError := ctx.Err(); contextError != nil {
		return nil, contextError
	}
	if err != nil {
		return nil, err
	}
	// New retains protobuf-backed schema pointers. Keep the immutable comparison
	// key separate so a TypeSystem getter cannot mutate the key after transfer.
	key := proto.Clone(body).(*openfgav1.AuthorizationModel)
	key.Id = ""
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	prepared := &BrowserModelValidation{body: key, validated: true}
	prepared.resolverTypesystem.Store(typesys)
	return prepared, nil
}

// InstallBrowserModelValidation publishes only a fully prepared result. The
// startup caller invokes it after its whole bounded phase succeeds. A different
// body cannot replace a live entry; installing the same body is harmless.
func InstallBrowserModelValidation(ctx context.Context, prepared *BrowserModelValidation) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if prepared == nil || !prepared.validated || prepared.body == nil ||
		prepared.body.GetId() != "" || len(prepared.body.GetConditions()) != 0 {
		return errors.New("browser model validation was not prepared")
	}
	for {
		if current := browserModelValidation.Load(); current != nil {
			if err := ctx.Err(); err != nil {
				return err
			}
			if !proto.Equal(current.body, prepared.body) {
				return errors.New("browser model validation is already installed for a different model")
			}
			return ctx.Err()
		}
		if err := ctx.Err(); err != nil {
			return err
		}
		if browserModelValidation.CompareAndSwap(nil, prepared) {
			if err := ctx.Err(); err != nil {
				// Roll back only our own publication, never another entry.
				browserModelValidation.CompareAndSwap(prepared, nil)
				return err
			}
			return nil
		}
	}
}

func (prepared *BrowserModelValidation) matches(model *openfgav1.AuthorizationModel) bool {
	if prepared == nil || !prepared.validated || prepared.body == nil || model == nil {
		return false
	}
	// Do not copy protobuf MessageState or mutate/clone the incoming model.
	// Compare every schema field, including nested metadata and unknown fields,
	// while ignoring only Id. The descriptor-coverage test guards this view.
	body := &openfgav1.AuthorizationModel{
		SchemaVersion:   model.GetSchemaVersion(),
		TypeDefinitions: model.GetTypeDefinitions(),
		Conditions:      model.GetConditions(),
	}
	body.ProtoReflect().SetUnknown(model.ProtoReflect().GetUnknown())
	return proto.Equal(prepared.body, body)
}

// newAndValidateResolvedModel is called only by MemoizedTypesystemResolverFunc,
// after its existing datastore lookup and cache miss. It transfers one complete
// TypeSystem, never a struct copy or graphs shared by different model bindings.
func newAndValidateResolvedModel(ctx context.Context, storeID, modelID string, model *openfgav1.AuthorizationModel) (*TypeSystem, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	prepared := browserModelValidation.Load()
	if storeID == "" || modelID == "" || model == nil || model.GetId() != modelID || !prepared.matches(model) {
		return NewAndValidate(ctx, model)
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	typesys := prepared.resolverTypesystem.Swap(nil)
	if typesys == nil {
		return NewAndValidate(ctx, model)
	}
	// Cancellation after the claim discards the object. Never return it to the
	// slot, where another store/model request could claim the same mutable state.
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	typesys.modelID = modelID
	log.Print("[browser-openfga] model_resolver_transfer claimed=true")
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	return typesys, nil
}

// NewAndValidate keeps the original path for every uncached model. On an exact
// known body, New still builds fresh maps, graphs, conditions and local caches
// using the real model ID; only the already-proven schema validation is reused.
func NewAndValidate(ctx context.Context, model *openfgav1.AuthorizationModel) (*TypeSystem, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	var typesys *TypeSystem
	var err error
	if prepared := browserModelValidation.Load(); prepared.matches(model) {
		typesys, err = New(model)
	} else {
		typesys, err = newAndValidate(ctx, model)
	}
	if contextError := ctx.Err(); contextError != nil {
		return nil, contextError
	}
	return typesys, err
}

// ValidateBrowserAuthorizationModel is for a caller that only needs the error
// and would discard the TypeSystem. Request validation, size checks, generated
// IDs and the real datastore write remain the caller's unchanged responsibility.
func ValidateBrowserAuthorizationModel(ctx context.Context, model *openfgav1.AuthorizationModel) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if prepared := browserModelValidation.Load(); prepared.matches(model) {
		return ctx.Err()
	}
	_, err := newAndValidate(ctx, model)
	if contextError := ctx.Err(); contextError != nil {
		return contextError
	}
	return err
}
