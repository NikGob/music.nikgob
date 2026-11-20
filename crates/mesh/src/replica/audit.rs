use super::*;

// ------------------------------------- the copies of this computer's masters

/// Asks each owner device online, every [`AUDIT_EVERY`], which of this
/// computer's masters it holds, and keeps the `replica` rows as it says;
/// a change of who holds what wakes the worker's look at the masters that
/// may go. Only on a device with a window (where the owner decides).
pub(crate) async fn audit(sh: Arc<Shared>) {
    let Some(lib) = sh.library.clone() else { return };
    if sh.headless {
        return;
    }
    let mut next: HashMap<String, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(AUDIT_TICK).await;
        if sh.node.lock().is_none() {
            continue;
        }
        let peers = owner_peers(&sh);
        next.retain(|p, _| peers.contains(p));
        let mut changed = false;
        for peer in peers {
            if next.get(&peer).is_some_and(|t| Instant::now() < *t) {
                continue;
            }
            match audit_peer(&sh, &lib, &peer).await {
                Ok(c) => {
                    changed |= c;
                    next.insert(peer, Instant::now() + AUDIT_EVERY);
                }
                Err(e) => {
                    log::debug!("replica: the copies on {peer} not asked: {e}");
                    next.insert(peer, Instant::now() + AUDIT_RETRY);
                }
            }
        }
        if changed {
            sh.rt.send(Command::ReplicaCopiesChanged);
        }
    }
}

/// Asks `peer` about every master here, a page at a time. Whether who
/// holds what changed.
async fn audit_peer(sh: &Arc<Shared>, lib: &Library, peer: &str) -> Result<bool, String> {
    let mut after = String::new();
    let mut changed = false;
    loop {
        let hashes = {
            let (l, a) = (lib.clone(), after.clone());
            blocking(move || l.master_hashes_after(&a, PAGE)).await?
        };
        let Some(last) = hashes.last().cloned() else { break };
        let ask = CheckAsk { hashes, rehash: false };
        let a: CheckAnswer = post_json(sh, peer, "/v1/replica/check", &ask, ASK_WAIT).await?;
        // A file not read this time is not one gone: its row stays.
        let asked: Vec<String> = ask.hashes.into_iter().filter(|h| !a.deferred.contains(h)).collect();
        let (l, p) = (lib.clone(), peer.to_owned());
        changed |= blocking(move || l.set_node_replicas(&p, &asked, &a.held)).await?;
        after = last;
        tokio::time::sleep(PACE).await;
    }
    Ok(changed)
}

/// What the devices' answers confirm: for each hash, how many devices hold
/// it (each device once).
pub(crate) fn confirmed(answers: &[(String, Vec<Held>)], asked: &[String]) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for (_, held) in answers {
        let mine: HashSet<&str> = held.iter().map(|h| h.hash.as_str()).filter(|h| asked.iter().any(|a| a == h)).collect();
        for h in mine {
            *out.entry(h.to_owned()).or_default() += 1;
        }
    }
    out
}

/// The second look right before masters go: each trusted owner device
/// online (none of `leaving`) reads its copies of `hashes` again now; the
/// masters `need` devices still hold go back to the worker
/// ([`Command::ReplicaRechecked`]), the `replica` rows follow the answers.
pub(crate) async fn recheck(sh: Arc<Shared>, card: u64, hashes: Vec<String>, need: u32, leaving: Vec<String>) {
    let peers: Vec<String> = owner_peers(&sh).into_iter().filter(|p| !leaving.contains(p)).collect();
    let mut answers: Vec<(String, Vec<Held>)> = Vec::new();
    let mut failure: Option<String> = None;
    for peer in &peers {
        let mut held = Vec::new();
        let mut ok = true;
        // A batch first; a file it did not read (the budget ran out) is
        // asked again alone (a file larger than the budget is read so).
        let mut asks: VecDeque<Vec<String>> = hashes.chunks(RECHECK_BATCH).map(<[String]>::to_vec).collect();
        let mut alone: HashSet<String> = HashSet::new();
        while let Some(chunk) = asks.pop_front() {
            let ask = CheckAsk { hashes: chunk.clone(), rehash: true };
            match post_json::<CheckAnswer>(&sh, peer, "/v1/replica/check", &ask, RECHECK_WAIT).await {
                Ok(a) => {
                    let read: Vec<String> = chunk.iter().filter(|h| !a.deferred.contains(h)).cloned().collect();
                    if let Some(lib) = sh.library.clone() {
                        let (p, c, h) = (peer.clone(), read, a.held.clone());
                        if let Err(e) = blocking(move || lib.set_node_replicas(&p, &c, &h)).await {
                            log::warn!("replica: the copies on {peer} not noted: {e}");
                        }
                    }
                    for d in a.deferred.iter().filter(|d| chunk.contains(d)) {
                        if alone.insert(d.clone()) {
                            asks.push_back(vec![d.clone()]);
                        }
                    }
                    held.extend(a.held);
                }
                Err(e) => {
                    log::warn!("replica: {peer} did not read its copies again: {e}");
                    if failure.is_none() {
                        failure = Some(e);
                    }
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            answers.push((peer.clone(), held));
        }
    }
    let counts = confirmed(&answers, &hashes);
    let ok: Vec<String> = hashes.iter().filter(|h| counts.get(h.as_str()).copied().unwrap_or(0) >= need as usize).cloned().collect();
    let failed = if !ok.is_empty() {
        None
    } else if peers.is_empty() {
        Some("no device online".to_owned())
    } else {
        failure
    };
    sh.rt.send(Command::ReplicaRechecked { card, ok, failed });
}

// ------------------------------------------------ what the cards show

/// Counts, now and then, what each device of the ring holds of the files
/// its plan gives it ([`copy_stats`]), for the map's cards: a fallback for
/// devices that do not tell it themselves. Only on a device with a window
/// (where the map is); the ring holder's word is got from its presence
/// when it is not this device.
pub(crate) async fn count_copies(sh: Arc<Shared>) {
    let Some(lib) = sh.library.clone() else { return };
    if sh.headless {
        return;
    }
    tokio::time::sleep(COPIES_FIRST).await;
    loop {
        if sh.node.lock().is_some()
            && let Err(e) = count_copies_once(&sh, &lib).await
        {
            log::warn!("replica: the copies not counted: {e}");
        }
        tokio::time::sleep(COPIES_EVERY).await;
    }
}

/// What the ring holder gives the devices to store: this device's word
/// when it holds the ring, else the holder's heard on a link now (none:
/// the holder is away, and nothing is counted here).
fn holder_assign(sh: &Shared) -> Option<Vec<Assigned>> {
    if sh.state.lock().ring_key.is_some() {
        return Some(sh.presence.borrow().assign.clone());
    }
    owner_presences(sh).into_iter().find(|p| p.holder).map(|p| p.assign)
}

/// One count: every owner device the holder gave a storing profile, what
/// it holds of its plan and the plan's bytes ([`copy_stats`]) — a fallback
/// for devices whose build does not tell their progress itself. The files
/// are the ones this device's owner peers online offer (as the puller
/// plans from them), this device's own too.
async fn count_copies_once(sh: &Arc<Shared>, lib: &Library) -> Result<(), String> {
    let me = sh.key.public().to_string();
    let owners: Vec<String> = {
        let s = sh.state.lock();
        s.ring.as_ref().map(|r| r.others().filter(|c| c.scope == MeshScope::Owner).map(|c| c.node.clone()).collect()).unwrap_or_default()
    };
    let peers: Vec<String> = std::iter::once(me).chain(owner_peers(sh)).collect();
    let Some(assign) = holder_assign(sh) else {
        sh.set_copies(HashMap::new());
        return Ok(());
    };
    let mut copies: HashMap<String, MeshCopy> = HashMap::new();
    for a in assign.iter().filter(|a| owners.contains(&a.node)) {
        let from: Vec<String> = peers.iter().filter(|p| **p != a.node).cloned().collect();
        let (l, node, profile) = (lib.clone(), a.node.clone(), a.profile);
        if let Some(c) = blocking(move || copy_stats(&l, &node, profile, &from)).await? {
            copies.insert(a.node.clone(), c);
        }
    }
    sh.set_copies(copies);
    Ok(())
}

