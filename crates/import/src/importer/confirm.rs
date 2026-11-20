use super::*;

impl Importer {
    /// Parks a file that may be the stream track `track` (its length is
    /// off): option 1 attaches it to that track; the file's own tags
    /// import it as a track of its own.
    pub(super) fn park_stream(&self, ctx: &Ctx, analyzed: Analyzed, identity: Identity, track: &Track) -> FileOutcome {
        let lib = &self.inner.library;
        let album = track
            .release_id
            .and_then(|r| lib.get_release(r).ok().flatten())
            .map(|r| r.title);
        let choice = Choice::Stream {
            track: track.id,
            title: track.title.clone(),
            artist: track.artist_credit.display(),
            album,
            track_no: (track.track_no > 0).then_some(track.track_no),
            duration_ms: track.duration_ms,
            recording_mbid: track.recording_mbid.clone(),
        };
        self.park(ctx, analyzed, identity, vec![Choice::Tags, choice])
    }

    /// Puts a file into «Подтверди», in memory and in the database.
    pub(super) fn park(&self, ctx: &Ctx, analyzed: Analyzed, base: Identity, choices: Vec<Choice>) -> FileOutcome {
        let tags = TagView {
            title: analyzed.tags.title.clone(),
            artist: analyzed.tags.artist.clone(),
            album: analyzed.tags.album.clone(),
            year: analyzed.tags.year,
            track_no: analyzed.tags.track_no,
            duration_ms: Some(analyzed.info.duration_ms()),
            quality: Some(nikgob_core::i18n::status::SavedQuality::Data(quality_line(&analyzed))),
        };
        let payload = Payload {
            origin: analyzed.origin.clone(),
            targets: (*analyzed.targets).clone(),
            tags,
            choices,
            job_started: Some(ctx.started),
        };
        let path = analyzed.path.clone();
        let wishlist = analyzed.targets.wishlist;
        let stored = serde_json::to_string(&payload)
            .map_err(|e| ImportError::Other(e.to_string()))
            .and_then(|json| Ok(self.inner.library.put_confirm_item(&path.to_string_lossy(), wishlist, &json)?));
        let id = match stored {
            Ok(id) => ItemId(id as u64),
            Err(e) => {
                log::warn!("«Подтверди» for {} is not stored: {e}", path.display());
                ItemId(self.inner.next_item.fetch_add(1, Ordering::Relaxed))
            }
        };
        let item = ConfirmItem {
            id,
            job: ctx.job,
            path: path.clone(),
            tag_title: payload.tags.title.clone(),
            tag_artist: payload.tags.artist.clone(),
            tag_album: payload.tags.album.clone(),
            duration_ms: payload.tags.duration_ms,
            options: payload.choices.iter().map(|c| c.option(&payload.tags, &path)).collect(),
            wishlist,
            quality: payload.tags.quality.clone(),
        };
        lock(&self.inner.pending).insert(
            id,
            Parked {
                ctx: Some(ctx.clone()),
                job_started: ctx.started,
                origin: payload.origin,
                targets: analyzed.targets.clone(),
                read: Some((analyzed, base)),
                choices: payload.choices,
                item: item.clone(),
            },
        );
        ctx.emit(ImportEvent::NeedsConfirmation { item });
        FileOutcome::Parked(id)
    }

    /// The «Подтверди» queue of an earlier run, from the database (a file
    /// that is gone leaves it).
    pub(super) fn restore_pending(&self) {
        let lib = &self.inner.library;
        let rows = match lib.confirm_items() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("«Подтверди» not restored: {e}");
                return;
            }
        };
        for row in rows {
            let path = PathBuf::from(&row.path);
            let payload = serde_json::from_str::<Payload>(&row.payload).ok();
            let Some(p) = payload.filter(|_| long_path(&path).exists()) else {
                let _ = lib.delete_confirm_item(row.id);
                continue;
            };
            let id = ItemId(row.id as u64);
            let item = ConfirmItem {
                id,
                job: JobId(0),
                path: path.clone(),
                tag_title: p.tags.title.clone(),
                tag_artist: p.tags.artist.clone(),
                tag_album: p.tags.album.clone(),
                duration_ms: p.tags.duration_ms,
                options: p.choices.iter().map(|c| c.option(&p.tags, &path)).collect(),
                wishlist: row.wishlist,
                quality: p.tags.quality.clone(),
            };
            lock(&self.inner.active).insert(path);
            lock(&self.inner.pending).insert(
                id,
                Parked {
                    ctx: None,
                    // A row from before the start was kept: nothing counts
                    // as settled, so a file is only asked about, never
                    // refused.
                    job_started: p.job_started.unwrap_or(0),
                    origin: p.origin,
                    targets: Arc::new(p.targets),
                    read: None,
                    choices: p.choices,
                    item,
                },
            );
        }
    }

    /// Continues a parked import: `Some(i)` takes option `i` (0 is the
    /// file's own tags), `None` imports with the file's own tags. A file
    /// parked in an earlier run is read again first.
    pub async fn resolve(&self, item: ItemId, choice: Option<usize>) -> Result<FileOutcome> {
        let chosen = {
            let pending = lock(&self.inner.pending);
            let parked = pending
                .get(&item)
                .ok_or_else(|| ImportError::Other(format!("no pending item {}", item.0)))?;
            match choice {
                None => Choice::Tags,
                Some(i) => parked
                    .choices
                    .get(i)
                    .cloned()
                    .ok_or_else(|| ImportError::Other(format!("option {i} does not exist")))?,
            }
        };
        let Some(parked) = lock(&self.inner.pending).remove(&item) else {
            return Err(ImportError::Other(format!("item {} was resolved already", item.0)));
        };
        if let Err(e) = self.inner.library.delete_confirm_item(item.0 as i64) {
            log::warn!("«Подтверди» row {} stays: {e}", item.0);
        }
        let path = parked.item.path.clone();
        let ctx = parked
            .ctx
            .clone()
            .unwrap_or_else(|| self.detached_ctx(parked.targets.clone(), parked.job_started));
        let result = async {
            let (mut analyzed, base) = match parked.read {
                Some(r) => r,
                None => {
                    let (inner, p, o) = (self.inner.clone(), path.clone(), parked.origin.clone());
                    let mut a = blocking(move || analyze(&inner.cfg, &p, o)).await?;
                    a.targets = parked.targets.clone();
                    a.force_track = parked.targets.track_of(&path);
                    a.job_started = ctx.started;
                    let mut base = identity_from_tags(&a.tags, &path);
                    if let Some(t) = a.force_track
                        && let Some(id) = identity_of_track(&self.inner.library, t, &base)
                    {
                        base = id;
                    }
                    (a, base)
                }
            };
            let identity = match &chosen {
                Choice::Tags => base,
                Choice::Candidate(c) => apply_candidate(&base, &c.candidate()),
                Choice::Stream { track, .. } => {
                    analyzed.force_track = Some(*track);
                    base
                }
            };
            self.finish_import(&ctx, analyzed, identity).await
        }
        .await;
        lock(&self.inner.active).remove(&path);
        match result {
            Ok(o) => Ok(o),
            Err(e) => {
                self.failed(&ctx, &path, &e);
                Err(e)
            }
        }
    }

    /// Events of a file answered after a restart: no job listens, the
    /// broadcast does. `started`: when the file's own job started, so its
    /// album-mates that came with it are not taken as settled.
    fn detached_ctx(&self, targets: Arc<ImportTargets>, started: i64) -> Ctx {
        let (tx, _) = mpsc::unbounded_channel();
        Ctx {
            job: JobId(self.inner.next_job.fetch_add(1, Ordering::Relaxed)),
            tx,
            events: self.inner.events.clone(),
            targets,
            started,
            cue: Arc::default(),
        }
    }
}
