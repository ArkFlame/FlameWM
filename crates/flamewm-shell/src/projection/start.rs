//! Start menu projection: visible results, category filter, error line and slot lookups.

use super::*;

/// The single Start result owner: every projection and click path reads this.
/// A non-empty query searches ALL categories (the category rail is hidden);
/// an empty query shows the selected presentation category.
#[must_use]
pub fn start_visible_results<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<&'a flamewm_api::applications::DesktopApplication> {
    if !query.trim().is_empty() {
        return model.query_view(query);
    }
    let base = model.results_view();
    match category {
        StartCategory::All => base,
        StartCategory::Power => Vec::new(),
        _ => base
            .into_iter()
            .filter(|application| application_matches_category(application, category))
            .collect(),
    }
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    // Search mode hides the whole category rail (incl. Power) and searches all.
    let searching = !query.trim().is_empty();
    document.visible("start-categories", !searching)?;
    // A search ignores the category, so one query serves every rail check and the count.
    let results = start_visible_results(model, category, query);
    for (name, value) in PRESENTATION_CATEGORIES {
        let visible = match value {
            StartCategory::Power => session_available(*session),
            _ if value == category => true,
            _ if searching => !results.is_empty(),
            _ => !start_visible_results(model, value, query).is_empty(),
        };
        document.visible(&format!("start-category-{name}"), visible)?;
    }
    let count = results.len();
    document.text(
        "start-search-label",
        if query.trim().is_empty() {
            "Search applications".to_owned()
        } else {
            format!("{query} ({count})")
        },
    )?;
    Ok(())
}

/// Start error line. Markup must provide this node (hidden by default).
pub const START_STATUS_NODE: &str = "start-status-label";

/// Project the Start error line: shown with `message`, hidden when `None`.
pub fn project_start_error(
    document: &mut impl UiDocumentAccess,
    message: Option<&str>,
) -> Result<(), String> {
    match message {
        Some(message) => {
            document.text_ref(START_STATUS_NODE, message)?;
            document.visible(START_STATUS_NODE, true)
        }
        None => document.visible(START_STATUS_NODE, false),
    }
}

#[must_use]
pub fn start_application_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<flamewm_api::DesktopAppId> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.id.clone())
}

/// User-visible name of the application in `slot`, for Start error text.
#[must_use]
pub fn start_application_name_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<String> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.name.clone())
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    let bucket = match category {
        StartCategory::All => return true,
        StartCategory::Power => return false,
        StartCategory::Development => StartBucket::Development,
        StartCategory::Games => StartBucket::Games,
        StartCategory::Graphics => StartBucket::Graphics,
        StartCategory::Internet => StartBucket::Internet,
        StartCategory::Multimedia => StartBucket::Multimedia,
        StartCategory::System => StartBucket::System,
        StartCategory::Utilities => StartBucket::Utilities,
    };
    bucket_for_application(application) == bucket
}

#[cfg(test)]
mod category_tests;
