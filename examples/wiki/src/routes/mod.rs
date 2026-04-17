pub mod pages;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_generates_html() {
        use autumn_web::prelude::*;
        let markup = pages::layout("Test Title", html! { p { "Test Content" } });
        let html_string = markup.into_string();
        assert!(html_string.contains("Test Title — Wiki"));
        assert!(html_string.contains("Test Content"));
    }

    #[test]
    fn test_status_badge() {
        let markup = pages::status_badge("published");
        let html_string = markup.into_string();
        assert!(html_string.contains("published"));
        assert!(html_string.contains("bg-green-100"));

        let markup = pages::status_badge("draft");
        let html_string = markup.into_string();
        assert!(html_string.contains("draft"));
        assert!(html_string.contains("bg-yellow-100"));

        let markup = pages::status_badge("archived");
        let html_string = markup.into_string();
        assert!(html_string.contains("archived"));
        assert!(html_string.contains("bg-gray-200"));
    }

    #[test]
    fn test_redirect_to() {
        let markup = pages::redirect_to("/pages/hello");
        let html_string = markup.into_string();
        assert!(html_string.contains("url=/pages/hello"));
        assert!(html_string.contains("Redirecting"));
    }

    #[test]
    fn test_new_form() {
        let _runtime = tokio::runtime::Runtime::new().unwrap();
        let markup = _runtime.block_on(async { pages::new_form().await });
        let html_string = markup.into_string();
        assert!(html_string.contains("New Page"));
    }

    #[test]
    fn test_page_form_into_update() {
        use crate::routes::pages::PageForm;
        let form = PageForm {
            title: "Test Title".into(),
            body: "Test Body".into(),
            status: "published".into(),
        };

        let update_page = form.into_update();
        if let autumn_web::Patch::Set(title) = update_page.title {
            assert_eq!(title, "Test Title");
        } else {
            panic!("Expected title to be Set");
        }
    }
}
