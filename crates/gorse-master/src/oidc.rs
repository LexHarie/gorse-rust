// Ports: master/master.go (OIDC setup in `Serve`), master/rest.go (`handleOAuth2Callback`, `checkLogin`'s OIDC branch, `handleUserInfo`) @ add21b5

//! OpenID Connect login for the dashboard (`master/master.go` and
//! `master/rest.go`).
//!
//! Go discovers the provider when the master serves, then keeps the ID token of
//! each login in a cache until the token expires. The dashboard checks the
//! `id_token` cookie against that cache. This module does the same with
//! `openidconnect`, and keeps the cache with `moka`, where each entry lives as
//! long as its token does.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine as _;
use gorse_config::config::schema::OIDCConfig;
use moka::Expiry;
use moka::sync::Cache;
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::reqwest::redirect::Policy;
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    RedirectUrl, Scope, TokenResponse,
};

use crate::Master;
use crate::rest::auth;
use crate::rest::dashboard_api::UserInfo;

/// The scopes the login asks for beyond `openid` (Go's `oauth2Config.Scopes`).
const SCOPES: [&str; 2] = ["profile", "email"];

/// A login: the raw ID token, and when it expires as Unix seconds.
pub(crate) struct Login {
    /// The raw ID token, which the `id_token` cookie holds.
    pub(crate) raw_id_token: String,
    /// The expiry of the ID token (Go's `idToken.Expiry`).
    pub(crate) expires_at: i64,
}

/// What the token cache keeps for a logged-in token.
#[derive(Clone)]
struct CachedUser {
    /// The claims of the user (Go's `UserInfo`).
    info: UserInfo,
    /// How long the entry lives: the time until the token expires.
    ttl: Duration,
}

/// Gives each cache entry the lifetime of its token (Go's `time.Until(idToken.Expiry)`).
struct TokenTtl;

impl Expiry<String, CachedUser> for TokenTtl {
    fn expire_after_create(
        &self,
        _token: &String,
        value: &CachedUser,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }
}

/// The OIDC state of a master whose provider was discovered. The client is built
/// from these parts where it is used, since its type depends on which endpoints
/// it has.
pub(crate) struct Oidc {
    /// The provider's metadata (Go's `provider`).
    metadata: CoreProviderMetadata,
    /// The client's ID, secret and redirect URL (Go's `oauth2Config`).
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect: RedirectUrl,
    /// The HTTP client the discovery and the token exchange use. Redirects are not
    /// followed, as Go's HTTP client is set up.
    http: openidconnect::reqwest::Client,
    /// The logged-in tokens (Go's `tokenCache`).
    tokens: Cache<String, CachedUser>,
}

/// Builds the client of an [`Oidc`] state, with the redirect URL set.
macro_rules! client {
    ($oidc:expr) => {
        CoreClient::from_provider_metadata(
            $oidc.metadata.clone(),
            $oidc.client_id.clone(),
            Some($oidc.client_secret.clone()),
        )
        .set_redirect_uri($oidc.redirect.clone())
    };
}

impl Oidc {
    /// Discovers the provider of `cfg` (Go's `oidc.NewProvider`).
    pub(crate) async fn discover(cfg: &OIDCConfig) -> Result<Self, String> {
        let http = openidconnect::reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|err| err.to_string())?;
        let issuer = IssuerUrl::new(cfg.issuer.clone()).map_err(|err| err.to_string())?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|err| err.to_string())?;
        let redirect = RedirectUrl::new(cfg.redirect_url.clone()).map_err(|err| err.to_string())?;
        Ok(Self {
            metadata,
            client_id: ClientId::new(cfg.client_id.clone()),
            client_secret: ClientSecret::new(cfg.client_secret.clone()),
            redirect,
            http,
            tokens: Cache::builder().expire_after(TokenTtl).build(),
        })
    }

    /// Go's `oauth2Config.AuthCodeURL("")`: the URL the login redirects to.
    fn auth_code_url(&self) -> String {
        let client = client!(self);
        let mut request = client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            || CsrfToken::new(String::new()),
            || Nonce::new(String::new()),
        );
        for scope in SCOPES {
            request = request.add_scope(Scope::new(scope.to_owned()));
        }
        let (url, _, _) = request.url();
        url.to_string()
    }

    /// Go's `oauth2Config.Exchange` and the checks of `handleOAuth2Callback`:
    /// exchanges `code`, verifies the ID token, and caches its claims.
    async fn exchange(&self, code: &str) -> Result<Login, String> {
        let client = client!(self);
        let response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|err| err.to_string())?
            .request_async(&self.http)
            .await
            .map_err(|err| err.to_string())?;
        let id_token = response
            .id_token()
            .ok_or_else(|| "missing id_token".to_owned())?;
        let verifier = client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, |_: Option<&Nonce>| Ok(()))
            .map_err(|err| err.to_string())?;
        let raw_id_token = id_token.to_string();
        let expires_at = claims.expiration().timestamp();
        let info = user_info_of(&raw_id_token)?;
        let ttl = Duration::from_secs(u64::try_from(expires_at - now_unix()).unwrap_or(0));
        self.tokens.insert(raw_id_token.clone(), CachedUser { info, ttl });
        Ok(Login {
            raw_id_token,
            expires_at,
        })
    }
}

/// Reads the claims of a verified ID token into [`UserInfo`]: the payload
/// segment of the JWT, which Go's `idToken.Claims` decodes.
fn user_info_of(raw_id_token: &str) -> Result<UserInfo, String> {
    let payload = raw_id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| "malformed id_token".to_owned())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
}

/// The current Unix time in seconds.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
}

impl Master {
    /// Go's `m.oauth2Config.AuthCodeURL("")`, when OIDC is set up.
    pub(crate) fn oidc_auth_code_url(&self) -> Option<String> {
        self.oidc.get().map(Oidc::auth_code_url)
    }

    /// Whether `token` is a logged-in ID token (Go's `m.tokenCache.Get(token) != nil`).
    pub(crate) fn oidc_token_is_known(&self, token: &str) -> bool {
        self.oidc
            .get()
            .is_some_and(|oidc| oidc.tokens.get(token).is_some())
    }

    /// The claims of the user who sent the `id_token` cookie, when that token is
    /// logged in (Go's `handleUserInfo`).
    pub(crate) fn oidc_user_info(&self, headers: &HeaderMap) -> Option<UserInfo> {
        let oidc = self.oidc.get()?;
        let token = auth::id_token_cookie(headers)?;
        oidc.tokens.get(&token).map(|user| user.info)
    }

    /// Exchanges an authorization `code` for a login (Go's `handleOAuth2Callback`).
    pub(crate) async fn oidc_exchange(&self, code: &str) -> Result<Login, String> {
        let oidc = self
            .oidc
            .get()
            .ok_or_else(|| "OIDC is not set up".to_owned())?;
        oidc.exchange(code).await
    }
}
