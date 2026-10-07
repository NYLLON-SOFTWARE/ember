//! Shared controls for Basecoat pages, preserving the existing form builder's wire format.

use super::forms::FormWith;
use super::html::Html;
use super::tag::{Attrs, attrs};
use crate::ViewContext;

fn input_attrs(options: Attrs) -> Attrs {
    attrs().class("input").merge(options)
}

pub fn text_field(form: &FormWith, name: &str, value: Option<&str>, options: Attrs) -> Html {
    form.text_field(name, value, input_attrs(options))
}

pub fn email_field(form: &FormWith, name: &str, value: Option<&str>, options: Attrs) -> Html {
    form.email_field(name, value, input_attrs(options))
}

pub fn password_field(form: &FormWith, name: &str, options: Attrs) -> Html {
    form.password_field(name, input_attrs(options))
}

pub fn file_field(form: &FormWith, name: &str, options: Attrs) -> Html {
    form.file_field(name, input_attrs(options))
}

/// Keep the native details control in the tab order on redesigned forms.
pub fn translation_button(ctx: &ViewContext, key: &str) -> Html {
    super::translations::translation_button_with_summary(ctx, key, attrs().class("btn ui-translation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_preserve_form_names_escaping_and_upload_encoding() {
        let form = super::super::form_with("/first_run").model("user");
        let input = text_field(&form, "name", Some("<Admin & 'friends'>"), attrs().required(true));
        assert!(input.0.contains("class=\"input\""));
        assert!(input.0.contains("name=\"user[name]\""));
        assert!(input.0.contains("id=\"user_name\""));
        assert!(input.0.contains("value=\"&lt;Admin &amp; &#39;friends&#39;&gt;\""));
        assert!(input.0.contains("required=\"required\""));

        let password = password_field(&form, "password", attrs().maxlength(72).autocomplete("new-password"));
        assert!(password.0.contains("type=\"password\""));
        assert!(password.0.contains("maxlength=\"72\""));
        assert!(!password.0.contains("value="));

        let upload = file_field(&form, "avatar", attrs().accept("image/*"));
        let html = form.wrap(&upload.0);
        assert!(html.0.contains("enctype=\"multipart/form-data\""));
        assert!(html.0.contains("name=\"user[avatar]\""));
    }
}
