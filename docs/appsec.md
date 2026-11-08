# Application security: authorization boundaries

The main threat is a correctly authenticated user obtaining another actor's
resource or authority through a missing scope check, an overbroad permission,
a stale decision or an uncertain write. Rust memory safety does not establish
these authorization properties.

## Implemented practices

| Practice | Implementation and reason |
| --- | --- |
| Deny by default | Explicit route/action allowlists; errors never map to allow; `AUTHZ_MODE=deny` has no permissive fallback |
| Trusted actor | Server-side session resolves the user. DTOs reject extra actor/owner/admin fields; there is no arbitrary-principal Check API |
| Complete scope binding | Immutable organization/assigner links; typed HTTP IDs; SQL organization/task locks and revision checks around graph authorization |
| Least authority for delegation | Owner-only membership/ownership/organization permissions; task grant management requires can_delete. Reading/editing alone does not imply delegation |
| Subject validation | Known users only; ownership target must be an organization member. Explicit task grants to external users are allowed by Openlane. Raw FGA strings/usersets/wildcards are rejected |
| Service-level enforcement | Mutations authorize again while holding organization/task locks; a middleware decision is not reused as permission to mutate later |
| Transfer checks on both sides | Current SQL and FGA owner, exact expected owner and eligible new member; task organization/assigner cannot be moved |
| Atomic relationship replacement | Owner/role removal and addition share one OpenFGA Write; SQL journal and organization fence hide incomplete state and support replay |
| Fresh permission to access | Every allow requires live Check. Only denials may be cached; changed revisions discard delayed decisions |
| Fail-closed uncertainty | Durable global mutation fence blocks replicas and readiness after timeout, malformed acknowledgement or cancellation, including restart |
| Secure enumeration | Every returned task is checked live. Complete SQL candidate scan is bounded to 256 objects; oversized lists fail closed, not truncate silently |
| Verified migration | Validate source, freeze/bind it, checkpoint batches, compare tuples and positive/negative decisions before activation; never replay legacy ACL after cutover |
| Strict protocol parsing | Exact operation statuses, no redirects, limited JSON body/deadline, strict bool decision, duplicate/unknown fields rejected, tuple-page invariants |
| Browser boundary | No token in localStorage; clear old identity data, reject delayed old responses, use textContent, no-store and restrictive CSP |
| Audit signals | Structured operation/outcome events and bounded-cardinality metrics; credentials are excluded from logs and UI request history |

These follow the principles of [OWASP Authorization Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html)
and [OWASP Authorization Testing Automation](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Testing_Automation_Cheat_Sheet.html).
The implementation details come from this repository and the pinned
[Openlane organization/task sources](openlane-model.md).

## Why the Rust design matters

Typed HTTP identifiers reject raw graph syntax; service-level validators and
closed relation allowlists remain required at internal call sites. They do not
replace organization validation and concurrency checks in SQL.
`Result<bool, AuthError>` keeps an unavailable decision distinct from `false`,
and HTTP maps uncertainty to 503. `Authorizer::change` expresses a mixed atomic
relationship change so callers cannot accidentally split a transfer into two
independent operations. `Arc<dyn Authorizer>` lets transport tests inject errors
without introducing a permissive production provider.

Avoid `unwrap_or(true)`, positive-cache fallback, stringly typed arbitrary tuple
write APIs, checks on a parent without checking the actual requested scope,
and trusting client-supplied user/owner claims. Do not silently add a new route
to an authentication-only group without defining its authorization contract.

## Evidence to require for a new role or operation

Add expected allow and deny cases for actor × action × scope, including the
another organization's objects. Test direct and inherited grants, delegation
without data access, data access without delegation, revocation with the old
session, and invalid/unknown inputs. Test the list representation as well as
single-object access: an object's presence or metadata can leak through a list
even when its GET is protected.

Mutation tests should cover interruption before acknowledgement, after the
remote write but before acknowledgement, and after acknowledgement but before
SQL finalization. Check the graph, metadata, readiness and another replica.
A successful HTTP response alone is not sufficient evidence of revocation.

The migration suite uses real databases and the real OpenFGA decision engine;
the adapter suite controls wire responses for malformed and ambiguous cases.
Browser tests exercise real APIs and also verify stale-response disposal and
untrusted metadata rendering. See [verification](verification.md).

## Remaining production work

These are concrete boundaries, not claims already satisfied by the lab:

- **Identity and role lifecycle:** enterprise identity integration, disabled-user
  handling, session revocation across devices, role expiry, organization-membership
  removal and object cleanup are not a full implemented administration product.
- **Separation of duties:** the pinned model allows an owner/admin to delegate
  the actions it defines. There is no approval workflow, dual control, emergency
  access or blanket prohibition of all self-assignment. A different organization
  policy requires explicit model/API changes and new negative tests.
- **Tamper-resistant audit:** structured logs are useful evidence, but not an
  immutable audit ledger with retention, correlation IDs, access review and
  alerting. Do not describe them as a compliance audit system.
- **Operational authority:** isolate OpenFGA, authenticate service clients, use
  TLS where needed, restrict DB roles and prevent direct external tuple writers.
  A privileged administrator bypassing these boundaries is outside the protocol.
- **Resource abuse:** request/response/list bounds and login throttling are in
  place. Global/per-tenant request quotas, production pagination and production
  cleanup/backpressure policies need measured sizing. The organization-wide lock is
  deliberately conservative and may limit throughput.
- **Recovery and changes:** automate reconciliation only with a complete durable
  mutation journal and evidence that remote writes are drained. Current uncertain
  fences require explicit operator reconciliation. Model upgrades, source-schema
  evolution, store rotation and rollback need separate migration plans.
- **Independent assurance:** property-based fuzzing of the boundary, broader
  adversarial testing, long-running fault campaigns and review of deployment
  settings remain useful. The checked scenarios and finite Kani proofs do not
  establish absence of all authorization bugs.

Fail-closed is the recovery policy: reduced availability is acceptable; opening
access to make a failed test or incomplete migration look successful is not.
