//! `QrCodeController` (reference/app/controllers/qr_code_controller.rb): a QR code SVG for a
//! Base64url-encoded URL.

mod rqrcode;

use matchbox_kit::{Ctx, Error, ExpiresIn, Result, StatusCode};
use rails_compat::encoding;

use crate::concerns::{self, Before};

/// `allow_unauthenticated_access`
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    // `Base64.urlsafe_decode64(params[:id])` raises ArgumentError (a 500) on malformed input.
    let id = c.param_str("id").unwrap_or_default().to_string();
    let url = encoding::urlsafe_decode(&id).ok_or_else(|| Error::internal(anyhow::anyhow!("invalid base64")))?;
    // Too much to encode is the client's doing (rqrcode raises, a 500 in Rails).
    let qr_code = rqrcode::svg_bytes(&url).ok_or(Error::Status(StatusCode::UNPROCESSABLE_ENTITY))?;

    // `expires_in 1.year, public: true`
    c.expires_in(31_556_952, ExpiresIn { public: true, ..ExpiresIn::default() });
    Ok(c.render_as(StatusCode::OK, "image/svg+xml; charset=utf-8", qr_code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_ruby_urlsafe_decode64() {
        // `None` where Ruby raises ArgumentError.
        assert_eq!(encoding::urlsafe_decode("aHR0cDovL2NhbXBmaXJlLnRlc3Q").unwrap(), b"http://campfire.test");
        assert_eq!(encoding::urlsafe_decode("aHR0cDovL2NhbXBmaXJlLnRlc3Q=").unwrap(), b"http://campfire.test");
        assert_eq!(encoding::urlsafe_decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(encoding::urlsafe_decode("+/8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(encoding::urlsafe_decode(""), Some(vec![]));
        assert_eq!(encoding::urlsafe_decode("a"), None);
        assert_eq!(encoding::urlsafe_decode("ab="), None);
        assert_eq!(encoding::urlsafe_decode("ab=c"), None);
        assert_eq!(encoding::urlsafe_decode("aB=="), None);
        assert_eq!(encoding::urlsafe_decode("a*bc"), None);
    }
}
