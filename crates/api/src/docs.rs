//! `/openapi.json` (made from the routes themselves) and Swagger UI on
//! `/docs`, served from files built into the program: no token needed,
//! nothing fetched from the internet.

use axum::Router;
use utoipa::OpenApi;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "music.nikgob",
        version = "1",
        description = "Локальный API плеера. Каждый адрес /v1 требует токен: заголовок `Authorization: Bearer nk_…` (для /v1/events и /v1/media/… можно `?token=nk_…`). Токен создаётся в плеере: Настройки → Межсеть → API и MCP → «Новый токен». Время — в миллисекундах, громкость — от 0 до 1. Ошибка: `{\"error\", \"message\", \"details\"}`."
    ),
    paths(
        crate::routes::state,
        crate::routes::play,
        crate::routes::pause,
        crate::routes::resume,
        crate::routes::toggle,
        crate::routes::next,
        crate::routes::previous,
        crate::routes::seek,
        crate::routes::mode,
        crate::routes::volume,
        crate::routes::duck,
        crate::routes::unduck,
        crate::routes::list_outputs,
        crate::routes::set_output,
        crate::routes::queue,
        crate::routes::queue_add,
        crate::routes::queue_play,
        crate::routes::queue_remove,
        crate::routes::queue_move,
        crate::routes::queue_clear,
        crate::events::events,
        crate::library::search,
        crate::library::track,
        crate::library::album,
        crate::library::artist,
        crate::library::playlists,
        crate::library::playlist,
        crate::library::lyrics,
        crate::media::audio,
        crate::media::cover,
    ),
    components(schemas(
        crate::error::ErrorBody,
        crate::error::ErrorDetails,
        crate::state::PlayerState,
        crate::state::StateAnswer,
        crate::state::NowTrack,
        crate::state::NextTrack,
        crate::state::OutputRef,
        crate::state::DuckState,
        crate::state::PlayStatus,
        crate::state::RepeatMode,
        crate::play::Kind,
        crate::play::Mode,
        crate::play::Matched,
        crate::routes::PlayBody,
        crate::routes::PlayAnswer,
        crate::routes::VolumeBody,
        crate::routes::DuckBody,
        crate::routes::DuckAnswer,
        crate::routes::SeekBody,
        crate::routes::ModeBody,
        crate::routes::OutputBody,
        crate::routes::QueueItem,
        crate::routes::QueueAnswer,
        crate::routes::QueueAdd,
        crate::routes::QueueMode,
        crate::routes::QueueMove,
        crate::outputs::Output,
        crate::library::Track,
        crate::library::Album,
        crate::library::AlbumDetail,
        crate::library::Artist,
        crate::library::ArtistDetail,
        crate::library::Playlist,
        crate::library::PlaylistDetail,
        crate::library::SearchAnswer,
        crate::library::Lyrics,
        crate::library::LyricsLine,
        crate::library::LyricsWord,
    )),
    modifiers(&Security),
    tags(
        (name = "player", description = "Что играет, включить, пауза, громкость, приглушение, события"),
        (name = "queue", description = "Очередь"),
        (name = "outputs", description = "Куда идёт звук"),
        (name = "library", description = "Поиск и чтение библиотеки"),
        (name = "media", description = "Файлы песен и обложки"),
    )
)]
pub struct ApiDoc;

/// The token, in the header or (for the stream and the media) the query.
struct Security;

impl utoipa::Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme("bearer", SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)));
        components.add_security_scheme("token", SecurityScheme::ApiKey(ApiKey::Query(ApiKeyValue::new("token"))));
    }
}

/// `/docs` and `/openapi.json`.
pub fn swagger() -> Router {
    utoipa_swagger_ui::SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every route of the voice contract is in the document, with the
    /// token's schemes.
    #[test]
    fn the_document_has_the_routes() {
        let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let paths = doc["paths"].as_object().expect("paths");
        for p in [
            "/v1/state",
            "/v1/player/play",
            "/v1/player/pause",
            "/v1/player/resume",
            "/v1/player/toggle",
            "/v1/player/next",
            "/v1/player/previous",
            "/v1/player/volume",
            "/v1/player/duck",
            "/v1/events",
            "/v1/queue",
            "/v1/outputs",
            "/v1/player/output",
            "/v1/media/tracks/{id}/audio",
        ] {
            assert!(paths.contains_key(p), "{p}");
        }
        assert!(doc["paths"]["/v1/player/duck"]["delete"].is_object());
        assert!(doc["components"]["securitySchemes"]["bearer"].is_object());
        assert!(doc["components"]["schemas"]["PlayerState"].is_object());
    }
}
