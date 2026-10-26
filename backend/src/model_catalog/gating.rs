use super::*;

pub(super) const FAST_SERVICE_TIER_ID: &str = "priority";
pub(super) const FAST_SPEED_TIER_ID: &str = "fast";

/// Makes a previously generated catalog safe to reuse when the upstream model
/// cache cannot be refreshed. Older catalogs may still advertise capabilities
/// whose route settings have since changed.
pub(crate) fn prepare_cached_catalog_for_current_capabilities(
    home: &Path,
    native_web_search_models: &[String],
    image_detail_original_models: &[String],
    remote_compaction_models: &[String],
    codex_app_path: &str,
) -> Result<bool> {
    prepare_cached_catalog_in_context(
        home,
        native_web_search_models,
        image_detail_original_models,
        remote_compaction_models,
        &CatalogContext::new(codex_app_path),
    )
}

pub(crate) fn prepare_cached_catalog_in_context(
    home: &Path,
    native_web_search_models: &[String],
    image_detail_original_models: &[String],
    remote_compaction_models: &[String],
    context: &CatalogContext<'_>,
) -> Result<bool> {
    if !read_catalog_value(&home.join(relative_path()))
        .is_some_and(|value| snapshot_matches_runtime_source(&value, context.source()))
    {
        return Ok(false);
    }
    if !is_available(home) {
        return Ok(false);
    }

    let mut models = read_runtime_catalog_models(home)?;
    let original_models = models.clone();
    apply_remote_compaction_models(&mut models, remote_compaction_models);
    let allowed_model_keys = native_web_search_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let image_detail_original_model_keys = image_detail_original_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let mut changed = models != original_models;
    for model in &mut models {
        let previous = model.clone();
        gate_cached_native_web_search(model, &allowed_model_keys);
        gate_cached_image_detail_original(model, &image_detail_original_model_keys);
        prepare_cached_context_window(model);
        normalize_third_party_compaction_identity(model);
        changed |= *model != previous;
    }
    if changed {
        write_catalog(home, &models)?;
    }

    let written_models = read_runtime_catalog_models(home)?;
    let mut safely_gated_models = written_models.clone();
    apply_remote_compaction_models(&mut safely_gated_models, remote_compaction_models);
    for model in &mut safely_gated_models {
        gate_cached_native_web_search(model, &allowed_model_keys);
        gate_cached_image_detail_original(model, &image_detail_original_model_keys);
        prepare_cached_context_window(model);
        normalize_third_party_compaction_identity(model);
    }
    if safely_gated_models != written_models {
        bail!("复用的 Codey 模型目录仍包含与当前线路不匹配的模型能力");
    }
    Ok(true)
}

pub(super) fn apply_remote_compaction_models(models: &mut [Value], allowed: &[String]) {
    let allowed = allowed
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    for model in models {
        let enabled = model
            .get("slug")
            .and_then(Value::as_str)
            .is_some_and(|slug| allowed.contains(&model_id::key(slug)));
        // Codex 忽略扩展字段，Codey 的原生请求入口用它选择会话 Provider。
        model["codey_remote_compaction"] = json!(enabled);
    }
}

#[cfg(test)]
pub(super) fn declares_fast_speed_support(model: &Value) -> bool {
    model
        .get("service_tiers")
        .and_then(Value::as_array)
        .is_some_and(|tiers| {
            tiers
                .iter()
                .any(|tier| tier.get("id").and_then(Value::as_str) == Some(FAST_SERVICE_TIER_ID))
        })
        || model
            .get("additional_speed_tiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier.as_str() == Some(FAST_SPEED_TIER_ID))
            })
}

pub(super) fn add_fast_speed_controls(model: &mut Value) {
    let service_tiers = model.get_mut("service_tiers").and_then(Value::as_array_mut);
    if let Some(service_tiers) = service_tiers {
        if !service_tiers
            .iter()
            .any(|tier| tier.get("id").and_then(Value::as_str) == Some(FAST_SERVICE_TIER_ID))
        {
            service_tiers.push(json!({
                "id": FAST_SERVICE_TIER_ID,
                "name": "Fast",
                "description": "1.5x speed, increased usage"
            }));
        }
    } else {
        model["service_tiers"] = json!([{
            "id": FAST_SERVICE_TIER_ID,
            "name": "Fast",
            "description": "1.5x speed, increased usage"
        }]);
    }

    let speed_tiers = model
        .get_mut("additional_speed_tiers")
        .and_then(Value::as_array_mut);
    if let Some(speed_tiers) = speed_tiers {
        if !speed_tiers
            .iter()
            .any(|tier| tier.as_str() == Some(FAST_SPEED_TIER_ID))
        {
            speed_tiers.push(json!(FAST_SPEED_TIER_ID));
        }
    } else {
        model["additional_speed_tiers"] = json!([FAST_SPEED_TIER_ID]);
    }
}

pub(super) fn gate_synthetic_native_web_search(
    model: &mut Value,
    allowed_model_keys: &HashSet<String>,
) {
    if model.get("codey_source").and_then(Value::as_str) != Some("third_party") {
        return;
    }
    gate_cached_native_web_search(model, allowed_model_keys);
}

fn gate_cached_native_web_search(model: &mut Value, allowed_model_keys: &HashSet<String>) {
    let allowed = model
        .get("slug")
        .and_then(Value::as_str)
        .is_some_and(|slug| allowed_model_keys.contains(&model_id::key(slug)));
    let source_declares_search = model.get("supports_search_tool").and_then(Value::as_bool)
        == Some(true)
        && model
            .get("web_search_tool_type")
            .and_then(Value::as_str)
            .is_some_and(|tool_type| !tool_type.trim().is_empty());
    if allowed && source_declares_search {
        return;
    }
    if let Some(object) = model.as_object_mut() {
        object.remove("supports_search_tool");
        object.remove("web_search_tool_type");
    }
}

/// Chat Completions 与 Anthropic Messages 都没有 `input_image.detail=original`
/// 的对应字段，Codex 只会按运行时目录里声明的能力决定是否发出这个值。适配
/// 线路上必须清掉该声明，否则历史里已经存在的原图请求会被本地路由拒绝。
pub(super) fn gate_cached_image_detail_original(
    model: &mut Value,
    allowed_model_keys: &HashSet<String>,
) {
    let allowed = model
        .get("slug")
        .and_then(Value::as_str)
        .is_some_and(|slug| allowed_model_keys.contains(&model_id::key(slug)));
    if allowed {
        return;
    }
    if let Some(object) = model.as_object_mut() {
        object.remove("supports_image_detail_original");
    }
}
