//! End-to-end tests for the parameter verbs (`set_parameter`,
//! `get_parameter`) — the "everything is a parameter" plumbing.

mod common;

use std::error::Error;

use surrealdb::types::{Object, SurrealValue, Value};
use superx_kernel::{Kernel, KernelError};

async fn target_entity(kernel: &Kernel) -> Result<surrealdb::types::RecordId, Box<dyn Error>> {
    Ok(kernel.create_entity("node_kernel_module", "admin").await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_then_get_roundtrips_the_value() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = target_entity(&kernel).await?;

    kernel
        .set_parameter(
            target.clone(),
            "attr_capture_interval_secs",
            Value::Number(2.into()),
        )
        .await?;

    let value = kernel
        .get_parameter(target, "attr_capture_interval_secs")
        .await?;
    assert_eq!(value, Some(Value::Number(2.into())));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_twice_latest_value_wins() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = target_entity(&kernel).await?;

    kernel
        .set_parameter(
            target.clone(),
            "attr_projects_root",
            Value::String("/old/path".to_string()),
        )
        .await?;
    kernel
        .set_parameter(
            target.clone(),
            "attr_projects_root",
            Value::String("/new/path".to_string()),
        )
        .await?;

    let value = kernel.get_parameter(target, "attr_projects_root").await?;
    assert_eq!(value, Some(Value::String("/new/path".to_string())));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unset_parameter_returns_none() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = target_entity(&kernel).await?;

    // Entirely unseeded parameter type.
    let value = kernel
        .get_parameter(target.clone(), "attr_never_seeded")
        .await?;
    assert!(value.is_none());

    // Seeded type, but never set on THIS target.
    let other = target_entity(&kernel).await?;
    kernel
        .set_parameter(other, "attr_set_elsewhere", Value::Bool(true))
        .await?;
    let value = kernel.get_parameter(target, "attr_set_elsewhere").await?;
    assert!(value.is_none(), "parameters must not leak across targets");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parameter_history_is_append_only() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = target_entity(&kernel).await?;

    kernel
        .set_parameter(target.clone(), "attr_knob", Value::Number(1.into()))
        .await?;
    kernel
        .set_parameter(target.clone(), "attr_knob", Value::Number(2.into()))
        .await?;
    kernel
        .set_parameter(target.clone(), "attr_knob", Value::Number(3.into()))
        .await?;

    let knob_type = kernel.find_type("attr_knob").await?;
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query(
            "SELECT count() AS count FROM state_ledger \
             WHERE target = $target AND type = $type \
             GROUP ALL",
        )
        .bind(("target", target.clone()))
        .bind(("type", knob_type))
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(3));

    // And the current value is still the latest.
    let value = kernel.get_parameter(target, "attr_knob").await?;
    assert_eq!(value, Some(Value::Number(3.into())));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_parameter_payload_surfaces_error() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = target_entity(&kernel).await?;

    // A foreign write to the parameter's attribute chain that does not
    // carry the { value: … } shape.
    kernel
        .ensure_type_definition("attr_misused", "attribute", "working")
        .await?;
    let mut payload = Object::new();
    payload.insert("not_value".to_string(), Value::Bool(true));
    kernel
        .supersede_state(target.clone(), "attr_misused", Value::Object(payload))
        .await?;

    let err = kernel
        .get_parameter(target, "attr_misused")
        .await
        .expect_err("shape-violating payload must surface as an error");
    assert!(
        matches!(err, KernelError::Corrupt(_)),
        "expected Corrupt, got: {err:?}",
    );
    Ok(())
}
