//! End-to-end tests for the parameter verbs.

mod common;

use std::error::Error;

use surrealdb::types::{Object, SurrealValue, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_get_roundtrip() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = kernel.create_entity("node_kernel_module").await?;

    kernel
        .set_parameter(target.clone(), "attr_poll_interval_secs", Value::Number(2.into()))
        .await?;
    let got = kernel
        .get_parameter(target, "attr_poll_interval_secs")
        .await?;
    assert_eq!(got, Some(Value::Number(2.into())));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supersession_latest_wins_history_preserved() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = kernel.create_entity("node_kernel_module").await?;

    kernel
        .set_parameter(target.clone(), "attr_knob", Value::Number(1.into()))
        .await?;
    kernel
        .set_parameter(target.clone(), "attr_knob", Value::Number(2.into()))
        .await?;

    assert_eq!(
        kernel.get_parameter(target.clone(), "attr_knob").await?,
        Some(Value::Number(2.into())),
        "latest wins"
    );

    #[derive(SurrealValue)]
    struct CountRow {
        c: i64,
    }
    let rows: Vec<CountRow> = kernel
        .db()
        .query("SELECT count() AS c FROM state_ledger WHERE target = $t GROUP ALL")
        .bind(("t", target))
        .await?
        .take(0)?;
    assert_eq!(rows[0].c, 2, "both parameter writes preserved");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unset_parameter_is_none() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = kernel.create_entity("node_kernel_module").await?;
    assert!(kernel
        .get_parameter(target, "attr_never_set")
        .await?
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_parameter_payload_errors() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let target = kernel.create_entity("node_kernel_module").await?;

    // Seed the parameter type, then write a payload missing "value".
    kernel
        .set_parameter(target.clone(), "attr_shape", Value::Number(1.into()))
        .await?;
    let mut bad = Object::new();
    bad.insert("not_value".to_string(), Value::Number(9.into()));
    kernel
        .supersede_state(target.clone(), "attr_shape", Value::Object(bad))
        .await?;

    let err = kernel
        .get_parameter(target, "attr_shape")
        .await
        .expect_err("corrupt payload must surface");
    assert!(err.to_string().contains("corrupt"));
    Ok(())
}
