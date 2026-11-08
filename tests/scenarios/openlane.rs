//! Real Axum -> PostgreSQL -> OpenFGA scenarios for the adapted Openlane model.
//! Included by `system.rs`, so the existing HTTP/gRPC CI matrix runs every case.
use super::{Api, M, S, Stack, TestResult, client, create_input, expect, ids, proxy, task_input};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use tokio::{sync::Barrier, task::JoinSet};
use uuid::Uuid;

async fn create(api: &Api, token: &str, organization: &str) -> TestResult<String> {
    let body = expect(
        api.call(M::POST, "/tasks", token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&create_input(organization)),
        S::CREATED,
    )
    .await?;
    Ok(body["id"].as_str().ok_or("missing task id")?.to_owned())
}

async fn create_denied(api: &Api, token: &str, organization: &str) -> TestResult {
    expect(
        api.call(M::POST, "/tasks", token)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&create_input(organization)),
        S::FORBIDDEN,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn organization_roles_replace_without_creating_implicit_task_access() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let erin = stack.login(&client, &app, "erin").await?;

    for path in ["/organizations", "/organizations/acme/members"] {
        expect(api.request(M::GET, path), S::UNAUTHORIZED).await?;
    }
    let organizations = expect(api.call(M::GET, "/organizations", &alice), S::OK).await?;
    assert_eq!(ids(&organizations), BTreeSet::from(["acme"]));
    let organizations = expect(api.call(M::GET, "/organizations", &erin), S::OK).await?;
    assert_eq!(ids(&organizations), BTreeSet::from(["other"]));
    expect(
        api.call(M::GET, "/tasks/other-review", &alice),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::GET, "/tasks/access-review", &erin),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::GET, "/organizations/acme/members", &dave),
        S::FORBIDDEN,
    )
    .await?;
    create_denied(&api, &bob, "other").await?;
    create_denied(&api, &erin, "acme").await?;

    // The demo gives Bob an explicit read-all permission in addition to admin.
    // Remove only that grant: the real Openlane admin role does not imply it.
    expect(
        api.call(
            M::DELETE,
            "/organizations/acme/permissions/can_view_task/bob",
            &alice,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, "/tasks/access-review", &bob), S::FORBIDDEN).await?;
    let created = create(&api, &bob, "acme").await?;
    create_denied(&api, &carol, "acme").await?;

    // Authorization precedes target lookup; nonowners cannot enumerate accounts
    // or promote themselves by writing a member role or an inherited permission.
    for subject in ["bob", "nonexistent"] {
        for token in [&bob, &dave, &erin] {
            expect(
                api.call(
                    M::PUT,
                    &format!("/organizations/acme/members/{subject}"),
                    token,
                )
                .json(&json!({"role":"admin"})),
                S::FORBIDDEN,
            )
            .await?;
            expect(
                api.call(
                    M::PUT,
                    &format!("/organizations/acme/permissions/can_edit_task/{subject}"),
                    token,
                ),
                S::FORBIDDEN,
            )
            .await?;
        }
    }
    expect(
        api.call(M::DELETE, "/organizations/acme/members/alice", &alice),
        S::CONFLICT,
    )
    .await?;
    expect(
        api.call(M::PUT, "/organizations/acme/members/bob", &alice)
            .json(&json!({"role":"owner"})),
        S::BAD_REQUEST,
    )
    .await?;
    expect(
        api.call(M::PUT, "/organizations/acme/permissions/owner/bob", &alice),
        S::BAD_REQUEST,
    )
    .await?;

    let member = "/organizations/acme/members/bob";
    for _ in 0..2 {
        expect(
            api.call(M::PUT, member, &alice)
                .json(&json!({"role":"member"})),
            S::NO_CONTENT,
        )
        .await?;
    }
    create_denied(&api, &bob, "acme").await?;
    // Role demotion does not silently revoke a separate direct assigner edge.
    expect(
        api.call(M::PATCH, &format!("/tasks/{created}"), &bob)
            .json(&task_input()),
        S::NO_CONTENT,
    )
    .await?;
    let members = expect(
        api.call(M::GET, "/organizations/acme/members", &alice),
        S::OK,
    )
    .await?;
    let bob_member = members
        .as_array()
        .ok_or("member list")?
        .iter()
        .find(|member| member["user_id"] == "bob")
        .ok_or("Bob membership")?;
    assert_eq!(bob_member["role"], "member");
    assert_eq!(bob_member["is_owner"], false);
    expect(
        api.call(M::PUT, member, &alice)
            .json(&json!({"role":"admin"})),
        S::NO_CONTENT,
    )
    .await?;
    let second = create(&api, &bob, "acme").await?;
    assert_ne!(created, second);
    expect(api.call(M::DELETE, member, &alice), S::NO_CONTENT).await?;
    create_denied(&api, &bob, "acme").await?;
    expect(
        api.call(M::GET, "/organizations/acme/members", &bob),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::GET, &format!("/tasks/{created}"), &bob), S::OK).await?;
    stack.assert_single_owner(&client, "acme", "alice").await?;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn direct_grants_assignees_and_inherited_permissions_remain_distinct() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let erin = stack.login(&client, &app, "erin").await?;
    let id = create(&api, &bob, "acme").await?;
    let path = format!("/tasks/{id}");
    for token in [&carol, &dave, &erin] {
        expect(api.call(M::GET, &path, token), S::FORBIDDEN).await?;
    }
    // Organization ownership flows through parent_context into every task.
    let owner_view = expect(api.call(M::GET, &path, &alice), S::OK).await?;
    assert_eq!(owner_view["can_edit"], true);
    assert_eq!(owner_view["can_delete"], true);
    for relation in ["owner", "assigner", "parent_context"] {
        expect(
            api.call(M::PUT, &format!("{path}/relations/{relation}/dave"), &bob),
            S::BAD_REQUEST,
        )
        .await?;
    }

    for (relation, editable) in [("viewer", false), ("editor", true), ("assignee", true)] {
        let grant = format!("{path}/relations/{relation}/dave");
        for _ in 0..2 {
            expect(api.call(M::PUT, &grant, &bob), S::NO_CONTENT).await?;
        }
        let view = expect(api.call(M::GET, &path, &dave), S::OK).await?;
        assert_eq!(view["can_edit"], editable);
        assert_eq!(view["can_delete"], false);
        expect(
            api.call(M::PATCH, &path, &dave).json(&task_input()),
            if editable {
                S::NO_CONTENT
            } else {
                S::FORBIDDEN
            },
        )
        .await?;
        expect(api.call(M::DELETE, &path, &dave), S::FORBIDDEN).await?;
        for subject in ["carol", "nonexistent"] {
            expect(
                api.call(M::PUT, &format!("{path}/relations/viewer/{subject}"), &dave),
                S::FORBIDDEN,
            )
            .await?;
        }
        for _ in 0..2 {
            expect(api.call(M::DELETE, &grant, &bob), S::NO_CONTENT).await?;
        }
        expect(api.call(M::GET, &path, &dave), S::FORBIDDEN).await?;
    }

    let inherited_view = "/organizations/acme/permissions/can_view_task/dave";
    expect(api.call(M::PUT, inherited_view, &alice), S::NO_CONTENT).await?;
    let direct_view = format!("{path}/relations/viewer/dave");
    expect(api.call(M::PUT, &direct_view, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::DELETE, &direct_view, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &dave), S::OK).await?;
    expect(api.call(M::GET, "/tasks/other-review", &dave), S::FORBIDDEN).await?;
    expect(api.call(M::DELETE, inherited_view, &alice), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &dave), S::FORBIDDEN).await?;

    // Upstream parent_editor is deliberately part of task.deleter. Keep that
    // behavior rather than inventing a different policy under Openlane's name.
    let inherited_edit = "/organizations/acme/permissions/can_edit_task/dave";
    expect(api.call(M::PUT, inherited_edit, &alice), S::NO_CONTENT).await?;
    let view = expect(api.call(M::GET, &path, &dave), S::OK).await?;
    assert_eq!(view["can_edit"], true);
    assert_eq!(view["can_delete"], true);
    expect(
        api.call(M::PATCH, &path, &dave).json(&task_input()),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(M::PUT, &format!("{path}/relations/viewer/carol"), &dave),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::DELETE, inherited_edit, &alice), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &dave), S::FORBIDDEN).await?;
    expect(api.call(M::GET, &path, &carol), S::OK).await?;
    let inherited_delete = "/organizations/acme/permissions/can_delete_task/dave";
    expect(api.call(M::PUT, inherited_delete, &alice), S::NO_CONTENT).await?;
    let inherited = expect(api.call(M::GET, &path, &dave), S::OK).await?;
    assert_eq!(inherited["can_delete"], true);
    expect(api.call(M::DELETE, inherited_delete, &alice), S::NO_CONTENT).await?;
    expect(api.call(M::DELETE, &path, &dave), S::FORBIDDEN).await?;
    expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, &id).await?;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn blocked_preserves_the_upstream_owner_and_assigner_precedence() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let id = create(&api, &bob, "acme").await?;
    let path = format!("/tasks/{id}");
    expect(
        api.call(M::PUT, &format!("{path}/relations/assignee/dave"), &bob),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(M::PATCH, &path, &dave).json(&task_input()),
        S::NO_CONTENT,
    )
    .await?;
    let block = format!("{path}/relations/blocked/dave");
    expect(api.call(M::PUT, &block, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &dave), S::FORBIDDEN).await?;
    expect(
        api.call(M::PATCH, &path, &dave).json(&task_input()),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, &block, &dave), S::FORBIDDEN).await?;
    expect(api.call(M::DELETE, &block, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &dave), S::OK).await?;

    // blocked denies viewing even to an owner, but upstream can_delete bypasses
    // the edit exclusion and deletion itself is not subject to blocked.
    for (subject, token) in [("alice", &alice), ("bob", &bob)] {
        expect(
            api.call(
                M::PUT,
                &format!("{path}/relations/blocked/{subject}"),
                &alice,
            ),
            S::NO_CONTENT,
        )
        .await?;
        expect(api.call(M::GET, &path, token), S::FORBIDDEN).await?;
        expect(
            api.call(M::PATCH, &path, token).json(&task_input()),
            S::NO_CONTENT,
        )
        .await?;
    }
    expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, &id).await?;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn ownership_transfer_revokes_warm_inherited_access_on_both_replicas() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let first_api = Api::new(client.clone(), &first);
    let second_api = Api::new(client.clone(), &second);
    let alice = stack.login(&client, &first, "alice").await?;
    let bob = stack.login(&client, &first, "bob").await?;
    let carol = stack.login(&client, &first, "carol").await?;
    let owner = "/organizations/acme/owner";
    for (actor, expected, target, status) in [
        (&bob, "alice", "nonexistent", S::FORBIDDEN),
        (&alice, "bob", "carol", S::CONFLICT),
        (&alice, "alice", "alice", S::CONFLICT),
        (&alice, "alice", "nonexistent", S::NOT_FOUND),
        (&alice, "alice", "dave", S::BAD_REQUEST),
    ] {
        expect(
            api_transfer(&first_api, owner, actor, expected, target),
            status,
        )
        .await?;
        stack.assert_single_owner(&client, "acme", "alice").await?;
    }
    for api in [&first_api, &second_api] {
        for _ in 0..2 {
            let before = expect(api.call(M::GET, "/tasks/access-review", &alice), S::OK).await?;
            assert_eq!(before["can_delete"], true);
            let before = expect(api.call(M::GET, "/tasks/vendor-review", &bob), S::OK).await?;
            assert_eq!(before["can_delete"], false);
        }
    }
    expect(
        api_transfer(&first_api, owner, &alice, "alice", "bob"),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "bob").await?;
    for api in [&first_api, &second_api] {
        expect(
            api.call(M::GET, "/tasks/access-review", &alice),
            S::FORBIDDEN,
        )
        .await?;
        expect(
            api.call(M::PATCH, "/tasks/access-review", &alice)
                .json(&task_input()),
            S::FORBIDDEN,
        )
        .await?;
        let after = expect(api.call(M::GET, "/tasks/vendor-review", &bob), S::OK).await?;
        assert_eq!(after["can_delete"], true);
        // Alice is still the direct assigner of this one task.
        expect(api.call(M::GET, "/tasks/vendor-review", &alice), S::OK).await?;
        expect(
            api.call(M::PUT, "/organizations/acme/members/carol", &alice)
                .json(&json!({"role":"admin"})),
            S::FORBIDDEN,
        )
        .await?;
    }
    expect(
        api_transfer(&second_api, owner, &alice, "bob", "alice"),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api_transfer(&second_api, owner, &bob, "alice", "carol"),
        S::CONFLICT,
    )
    .await?;
    expect(
        api_transfer(&second_api, owner, &bob, "bob", "carol"),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "carol").await?;
    let organizations = expect(first_api.call(M::GET, "/organizations", &carol), S::OK).await?;
    assert_eq!(organizations[0]["owner_id"], "carol");
    first.stop().await?;
    second.stop().await?;
    Ok(())
}

fn api_transfer(
    api: &Api,
    path: &str,
    actor: &str,
    expected: &str,
    target: &str,
) -> reqwest::RequestBuilder {
    api.call(M::PUT, path, actor)
        .json(&json!({"expected_owner":expected,"new_owner":target}))
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn competing_owner_transfers_cannot_leave_two_owners() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let barrier = Arc::new(Barrier::new(2));
    let mut transfers = JoinSet::new();
    for target in ["bob", "carol"] {
        let request = api_transfer(&api, "/organizations/acme/owner", &alice, "alice", target);
        let barrier = barrier.clone();
        transfers.spawn(async move {
            barrier.wait().await;
            Ok::<_, reqwest::Error>((target, request.send().await?.status()))
        });
    }
    let mut winner = None;
    let mut completed = 0;
    while let Some(result) = transfers.join_next().await {
        let (target, status) = result??;
        completed += 1;
        if status == S::NO_CONTENT {
            assert!(
                winner.replace(target).is_none(),
                "two transfers both succeeded"
            );
        } else {
            assert!(
                matches!(status, S::FORBIDDEN | S::CONFLICT | S::SERVICE_UNAVAILABLE),
                "unexpected concurrent transfer status {status}"
            );
        }
    }
    assert_eq!(completed, 2);
    let winner = winner.ok_or("neither transfer succeeded")?;
    stack.assert_single_owner(&client, "acme", winner).await?;
    assert!(!stack.authorization_pending("acme").await?);
    let token = stack.login(&client, &app, winner).await?;
    expect(
        api.call(
            M::PUT,
            "/organizations/acme/permissions/can_view_task/dave",
            &token,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(
            M::PUT,
            "/organizations/acme/permissions/can_view_task/dave",
            &alice,
        ),
        S::FORBIDDEN,
    )
    .await?;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn uncertain_owner_transfer_fences_the_organization_and_replays_after_restart() -> TestResult
{
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let first = stack.app(&proxy.url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &first);
    let other_api = Api::new(client.clone(), &second);
    let alice = stack.login(&client, &first, "alice").await?;
    let bob = stack.login(&client, &first, "bob").await?;
    let erin = stack.login(&client, &first, "erin").await?;
    expect(
        other_api.call(M::GET, "/tasks/access-review", &alice),
        S::OK,
    )
    .await?;
    expect(
        other_api.call(M::GET, "/tasks/access-review", &alice),
        S::OK,
    )
    .await?;
    let mut pause = proxy.pause("write", false).await;
    let (transfer, fenced) = tokio::join!(
        expect(
            api_transfer(&api, "/organizations/acme/owner", &alice, "alice", "bob"),
            S::SERVICE_UNAVAILABLE
        ),
        async {
            pause.captured().await?;
            // The real atomic Write already succeeded; only its response is lost.
            stack.assert_single_owner(&client, "acme", "bob").await?;
            assert!(stack.authorization_pending("acme").await?);
            for actor in [&alice, &bob] {
                expect(
                    other_api.call(M::GET, "/tasks/access-review", actor),
                    S::SERVICE_UNAVAILABLE,
                )
                .await?;
                expect(
                    other_api
                        .call(M::PUT, "/organizations/acme/members/carol", actor)
                        .json(&json!({"role":"admin"})),
                    S::SERVICE_UNAVAILABLE,
                )
                .await?;
            }
            // A write whose outcome is unknown fences every replica/scope.
            expect(
                other_api.call(M::GET, "/tasks/other-review", &erin),
                S::SERVICE_UNAVAILABLE,
            )
            .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    );
    drop(pause);
    fenced?;
    transfer?;
    first.stop().await?;
    let recovered = stack.app(&proxy.url).await?;
    let recovered_api = Api::new(client.clone(), &recovered);
    assert!(!stack.authorization_pending("acme").await?);
    stack.assert_single_owner(&client, "acme", "bob").await?;
    for api in [&other_api, &recovered_api] {
        for token in [&alice, &bob] {
            expect(
                api.call(M::GET, "/tasks/access-review", token),
                S::SERVICE_UNAVAILABLE,
            )
            .await?;
        }
        expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    }
    expect(
        api_transfer(
            &recovered_api,
            "/organizations/acme/owner",
            &alice,
            "alice",
            "bob",
        ),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    // A second restart must not reinstall the original demo owner tuple.
    recovered.stop().await?;
    let restarted = stack.app(&stack.fga_url).await?;
    let restarted_api = Api::new(client.clone(), &restarted);
    expect(
        restarted_api.call(M::GET, "/tasks/access-review", &alice),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "bob").await?;
    expect(
        api_transfer(
            &restarted_api,
            "/organizations/acme/owner",
            &bob,
            "bob",
            "carol",
        ),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    second.stop().await?;
    restarted.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let drained = stack.app(&stack.fga_url).await?;
    let drained_api = Api::new(client.clone(), &drained);
    expect(
        drained_api.call(M::GET, "/tasks/access-review", &alice),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        drained_api.call(M::GET, "/tasks/access-review", &bob),
        S::OK,
    )
    .await?;
    expect(
        api_transfer(
            &drained_api,
            "/organizations/acme/owner",
            &bob,
            "bob",
            "carol",
        ),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "carol").await?;
    drained.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn buffered_old_write_cannot_cross_quarantine_into_a_new_owner_transfer() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let original = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &original);
    let alice = stack.login(&client, &original, "alice").await?;
    let bob = stack.login(&client, &original, "bob").await?;
    let carol = stack.login(&client, &original, "carol").await?;
    let mut pause = proxy.pause_before_request("write").await;
    let (attempt, intercepted) = tokio::join!(
        expect(
            api_transfer(&api, "/organizations/acme/owner", &alice, "alice", "bob"),
            S::SERVICE_UNAVAILABLE
        ),
        pause.captured(),
    );
    intercepted?;
    attempt?;
    assert!(stack.authorization_pending("acme").await?);
    assert_eq!(proxy.acknowledged_writes(), 0);
    stack.assert_single_owner(&client, "acme", "alice").await?;
    original.stop().await?;

    // Recovery goes directly to OpenFGA, while the original complete request
    // remains buffered independently of its cancelled client connection.
    let recovered = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &recovered);
    stack.assert_single_owner(&client, "acme", "bob").await?;
    expect(
        api.call(M::GET, "/tasks/access-review", &bob),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    expect(
        api_transfer(&api, "/organizations/acme/owner", &bob, "bob", "carol"),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(
        api.call(M::PATCH, "/tasks/access-review", &bob)
            .json(&task_input()),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(
        api.call(M::PUT, "/tasks/access-review/relations/viewer/dave", &bob),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;

    // Release and observe a real upstream acknowledgement for the old A->B.
    // A sleep or another health probe would not prove that it cannot arrive later.
    drop(pause);
    proxy.wait_idle().await?;
    assert_eq!(proxy.acknowledged_writes(), 1);
    stack.assert_single_owner(&client, "acme", "bob").await?;
    recovered.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;

    let resumed = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &resumed);
    expect(
        api_transfer(&api, "/organizations/acme/owner", &bob, "bob", "carol"),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "carol").await?;
    let task = expect(api.call(M::GET, "/tasks/access-review", &carol), S::OK).await?;
    assert_eq!(task["recovery_required"], false);
    assert_eq!(task["can_delete"], true);
    expect(
        api.call(M::PATCH, "/tasks/access-review", &bob)
            .json(&task_input()),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api_transfer(&api, "/organizations/acme/owner", &bob, "carol", "bob"),
        S::FORBIDDEN,
    )
    .await?;
    resumed.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn uncertain_creation_replays_without_duplicate_tasks_or_authority() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let bob = stack.login(&client, &app, "bob").await?;
    let key = Uuid::new_v4().to_string();
    let mut pause = proxy.pause("write", false).await;
    let (creation, captured) = tokio::join!(
        expect(
            api.call(M::POST, "/tasks", &bob)
                .header("Idempotency-Key", &key)
                .json(&create_input("acme")),
            S::SERVICE_UNAVAILABLE
        ),
        async {
            pause.captured().await?;
            assert!(stack.authorization_pending("acme").await?);
            expect(api.call(M::GET, "/tasks", &bob), S::SERVICE_UNAVAILABLE).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    );
    drop(pause);
    captured?;
    creation?;
    // The exact original request may reconcile its durable intent immediately,
    // but it must not report 201 while the uncertain-write fence remains.
    expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&create_input("acme")),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert!(!stack.authorization_pending("acme").await?);
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    app.stop().await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    assert!(!stack.authorization_pending("acme").await?);
    expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&create_input("acme")),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.call(M::GET, "/tasks", &bob), S::SERVICE_UNAVAILABLE).await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    app.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let first = expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&create_input("acme")),
        S::CREATED,
    )
    .await?;
    let second = expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&create_input("acme")),
        S::CREATED,
    )
    .await?;
    assert_eq!(first, second);
    let id = first["id"].as_str().ok_or("missing recovered task")?;
    let path = format!("/tasks/{id}");
    let task = expect(api.call(M::GET, &path, &bob), S::OK).await?;
    assert_eq!(task["assigner_id"], "bob");
    let tasks: Value = expect(api.call(M::GET, "/tasks", &bob), S::OK).await?;
    assert_eq!(
        tasks
            .as_array()
            .ok_or("task list")?
            .iter()
            .filter(|task| task["id"] == id)
            .count(),
        1
    );
    let mut deletion_pause = proxy.pause("write", false).await;
    let (deletion, captured) = tokio::join!(
        expect(api.call(M::DELETE, &path, &bob), S::SERVICE_UNAVAILABLE),
        deletion_pause.captured(),
    );
    captured?;
    deletion?;
    drop(deletion_pause);
    // The same actor's exact delete retry reconciles the tombstone, but must
    // remain 503 until the explicit drain procedure clears global uncertainty.
    expect(api.call(M::DELETE, &path, &bob), S::SERVICE_UNAVAILABLE).await?;
    assert!(!stack.authorization_pending("acme").await?);
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    stack.assert_no_tuples(&client, id).await?;
    app.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&create_input("acme")),
        S::GONE,
    )
    .await?;
    app.stop().await?;
    Ok(())
}
