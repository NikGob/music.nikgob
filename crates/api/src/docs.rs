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
        description = "Локальный API плеера. Каждый адрес /v1 требует токен: заголовок `Authorization: Bearer nk_…` (для /v1/events и /v1/media/… можно `?token=nk_…`). Токен создаётся в плеере: Настройки → Межсеть → API и MCP → «Новый токен». Время — в миллисекундах, громкость — от 0 до 1. Ошибка: `{\"error\", \"message\", \"details\"}`. Разрушительные запросы (теги, корзина, удаление из плейлиста, переименование и удаление плейлиста) без `confirm_token` ничего не меняют и отвечают планом и одноразовым `confirm_token` (5 минут, для тех же аргументов). MCP (Streamable HTTP) — `POST /mcp` с тем же токеном."
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
        crate::connect::snapshot,
        crate::connect::load,
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
        crate::auth::whoami,
        crate::mutate::feedback,
        crate::mutate::create_playlist,
        crate::mutate::add_to_playlist,
        crate::mutate::rename_playlist,
        crate::mutate::remove_from_playlist,
        crate::mutate::delete_playlist,
        crate::mutate::retag,
        crate::mutate::trash,
        crate::stats::listening,
        crate::stats::forgotten,
        crate::stats::health,
        crate::stats::similar,
        crate::discover::search,
        crate::discover::discography,
        crate::discover::share,
        crate::downloads::inspect,
        crate::downloads::sources,
        crate::downloads::enqueue,
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
        crate::outputs::OutputKind,
        crate::connect::PlayerSnapshot,
        crate::connect::LoadAnswer,
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
        crate::auth::TokenInfo,
        crate::confirm::PlanAnswer,
        crate::mutate::FeedbackBody,
        crate::mutate::DoneAnswer,
        crate::mutate::PlaylistAddBody,
        crate::mutate::RenameBody,
        crate::mutate::RemoveBody,
        crate::mutate::RetagBody,
        crate::mutate::TrashBody,
        crate::mutate::Destructive,
        crate::stats::ListeningAnswer,
        crate::stats::StatTrackOut,
        crate::stats::NamedCount,
        crate::stats::KeyCount,
        crate::stats::ForgottenAnswer,
        crate::stats::HealthAnswer,
        crate::stats::FormatOut,
        crate::stats::FakeOut,
        crate::stats::SimilarAnswer,
        crate::stats::Similar,
        crate::discover::DiscoverAnswer,
        crate::discover::ServiceAnswer,
        crate::discover::FoundTrack,
        crate::discover::FoundAlbum,
        crate::discover::FoundArtist,
        crate::discover::FoundPlaylist,
        crate::discover::DiscographyAnswer,
        crate::discover::ShareAnswer,
        crate::downloads::DownloadsAnswer,
        crate::downloads::JobOut,
        crate::downloads::ConfirmOut,
        crate::downloads::WishOut,
        crate::downloads::StatusCount,
        crate::downloads::SourcesAnswer,
        crate::downloads::EnqueueBody,
        crate::downloads::EnqueueAnswer,
    )),
    modifiers(&Security),
    tags(
        (name = "player", description = "Что играет, включить, пауза, громкость, приглушение, события"),
        (name = "queue", description = "Очередь"),
        (name = "outputs", description = "Куда идёт звук"),
        (name = "library", description = "Поиск и чтение библиотеки"),
        (name = "media", description = "Файлы песен и обложки"),
        (name = "library-edit", description = "Лайки, плейлисты, теги и корзина"),
        (name = "stats", description = "Аналитика прослушиваний и библиотеки"),
        (name = "discover", description = "Поиск в сервисах, дискография, ссылка «Поделиться»"),
        (name = "downloads", description = "Загрузки и «Подтверди»"),
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
            "/v1/token",
            "/v1/library/feedback",
            "/v1/library/retag",
            "/v1/library/trash",
            "/v1/playlists/{id}/tracks",
            "/v1/playlists/{id}/remove",
            "/v1/stats/listening",
            "/v1/stats/forgotten",
            "/v1/stats/health",
            "/v1/stats/similar",
            "/v1/discover/search",
            "/v1/discover/artists/{id}/discography",
            "/v1/discover/share",
            "/v1/downloads",
            "/v1/downloads/sources",
            "/v1/player/snapshot",
            "/v1/player/load",
        ] {
            assert!(paths.contains_key(p), "{p}");
        }
        assert!(!paths.contains_key("/v1/player/remote"), "between the devices only");
        assert!(doc["components"]["schemas"]["PlayerSnapshot"].is_object());
        assert!(doc["paths"]["/v1/playlists"]["post"].is_object());
        assert!(doc["paths"]["/v1/playlists/{id}"]["delete"].is_object());
        assert!(doc["paths"]["/v1/playlists/{id}"]["put"].is_object());
        assert!(doc["components"]["schemas"]["PlanAnswer"].is_object());
        assert!(doc["paths"]["/v1/player/duck"]["delete"].is_object());
        assert!(doc["components"]["securitySchemes"]["bearer"].is_object());
        assert!(doc["components"]["schemas"]["PlayerState"].is_object());
    }
}
