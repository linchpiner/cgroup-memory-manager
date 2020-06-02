#[macro_use] extern crate log;
#[macro_use] extern crate failure;

use std::collections::{HashSet, HashMap};
use std::path::Path;
use std::path::PathBuf;
use std::f64;
use std::time::{Duration, Instant};
use std::thread::sleep;
use ex::fs::{read_to_string, write};

use byte_unit::Byte;
use clap::Clap;
use env_logger as logger;
use failure::Error;
use walkdir::WalkDir;

#[derive(Clap)]
#[clap(version = "0.0.1", author = "Paul Linchpiner <paul@linchpiner.com>")]
struct Opts {
    /// Path to the parent cgroup
    #[clap(long, default_value = "/sys/fs/cgroup/memory/docker")]
    parent: String,
    /// Cache usage threshold in % of memory limit, or bytes (other units are also supported)
    #[clap(long, default_value = "25%")]
    threshold: String,
    /// How frequently to check cache usage for all cgroups, in seconds
    #[clap(long, default_value = "10")]
    interval: u64,
    /// The minimum time to wait between forcing page reclaim
    #[clap(long, default_value = "30")]
    cooldown: u64,
}

// Return all of the directories that are in the specified root and do not contain other
// directories.
fn get_dir_leaves(root: &PathBuf) -> Vec<PathBuf> {
    let mut leaves = Vec::new();
    let mut dirs = HashSet::new();
    let walker = WalkDir::new(root).contents_first(true);
    let walker = walker.into_iter().filter_entry(|e| e.path().is_dir());
    let walker = walker.filter_map(|e| e.ok());
    for entry in walker {
        let path = entry.into_path();
        if dirs.contains(&path) {
            continue
        }
        leaves.push(path.clone());
        for ancestor in path.ancestors() {
            dirs.insert(ancestor.to_path_buf());
        }
    }
    leaves
}

#[derive(Debug, PartialEq)]
enum Threshold {
    Bytes(u64),
    Percent(f64),
}

struct ReclaimState {
    last_seen: Option<Instant>,
    last_reclaimed: Option<Instant>,
    last_error: Option<Instant>,
}

struct ReclaimLoop {
    parent: PathBuf,
    threshold: Threshold,
    interval: u64,
    cooldown: u64,
}

#[derive(Debug)]
struct MemoryStats {
    pub limit: u64,
    pub cache: u64,
    pub rss: u64,
}

impl ReclaimLoop {
    fn start(&self) {
        info!("Parent: {}", &self.parent.display());
        info!("Threshold: {:?}, interval: {}s, cooldown: {}s",
            self.threshold,
            self.interval,
            self.cooldown);

        let interval_ms = 1000u128 * self.interval as u128;
        let mut states = HashMap::new();
        loop {
            let now = Instant::now();
            self.reclaim(&mut states);
            self.cleanup(&now, &mut states);
            let elapsed = now.elapsed().as_millis();
            if elapsed > interval_ms {
                warn!("Reclaim loop took {}ms, longer than interval {}ms", elapsed, interval_ms);
            } else {
                let sleep_duration = (interval_ms - elapsed) as u64;
                let sleep_duration = Duration::from_millis(sleep_duration);
                sleep(sleep_duration);
            }
        }
    }

    fn reclaim(&self, states: &mut HashMap<PathBuf, ReclaimState>) {
        let cgroups = get_dir_leaves(&self.parent);
        for cgroup in &cgroups {

            let state = states.entry(cgroup.clone()).or_insert_with(|| {
                info!("New cgroup: {}", cgroup.display());
                ReclaimState {
                    last_seen: None,
                    last_reclaimed: None,
                    last_error: None,
                }
            });

            let now = Some(Instant::now());
            match self.reclaim_cgroup(cgroup, state) {
                Ok(()) => {
                    state.last_error = None;
                },
                Err(err) => {
                    if state.last_error.is_none() {
                        warn!("Failed to reclaim {}: {}", cgroup.display(), err);
                    }
                    state.last_error = now;
                }
            };
            state.last_seen = now;
        }
    }

    fn reclaim_cgroup(&self, path: &Path, state: &mut ReclaimState) -> Result<(), Error> {
        let stats = &get_memory_stats(path)?;
        if self.can_be_reclaimed(stats, state) {
            let display = path.display();
            info!("Reclaiming {}: {:?}", display, stats);
            reclaim(path)?;
            state.last_reclaimed = Some(Instant::now());
            let stats_after = &get_memory_stats(path)?;
            info!("Reclaimed  {}: {:?}", display, stats_after);
        }
        Ok(())
    }

    fn can_be_reclaimed(&self, stats: &MemoryStats, state: &ReclaimState) -> bool {
        if self.needs_to_be_reclaimed(stats) {
            let now = Instant::now();
            return match state.last_reclaimed {
                Some(last_reclaimed) => {
                    now.duration_since(last_reclaimed).as_secs() > self.cooldown
                },
                None => true,
            }
        }
        false
    }

    fn needs_to_be_reclaimed(&self, stats: &MemoryStats) -> bool {
        match self.threshold {
            Threshold::Bytes(threshold) => {
                stats.cache >= threshold
            },
            Threshold::Percent(threshold) => {
                stats.limit > 0 && stats.cache as f64 >= stats.limit as f64 * (threshold / 100f64)
            },
        }
    }

    fn cleanup(&self, now: &Instant, states: &mut HashMap<PathBuf, ReclaimState>) {
        states.retain(|cgroup, state| {
            if let Some(last_seen) = state.last_seen {
                if last_seen  >= *now {
                    return true;
                }
            }
            info!("Old cgroup: {}", cgroup.display());
            false
        });
    }
}

fn get_memory_stats(path: &Path) -> Result<MemoryStats, Error> {
    let limit_path = path.to_path_buf().join("memory.limit_in_bytes");
    let stats_path = path.to_path_buf().join("memory.stat");

    let mut rss: Option<u64> = None;
    let mut cache: Option<u64> = None;

    let string = read_to_string(&stats_path)?;
    for line in string.lines() {
        if rss.is_none() {
            rss = parse_u64_strip_prefix("rss ", line);
        }
        if cache.is_none() {
            cache = parse_u64_strip_prefix("cache ", line);
        }
        if rss.is_some() && cache.is_some() {
            break;
        }
    }

    let string = read_to_string(limit_path)?;
    let limit: Option<u64> = string.trim().parse().ok();

    Ok(MemoryStats {
        rss: rss.unwrap_or_default(),
        cache: cache.unwrap_or_default(),
        limit: limit.unwrap_or_default(),
    })
}

fn parse_u64_strip_prefix(prefix: &str, line: &str) -> Option<u64> {
    let line = line.trim();
    if line.starts_with(prefix) {
        return line.trim_start_matches(prefix).parse().ok()
    }
    None
}

fn reclaim(path: &Path) -> Result<(), Error> {
    let force_empty_path = path.to_path_buf().join("memory.force_empty");
    Ok(write(force_empty_path, "1")?)
}

fn get_parent(value: &str) -> Result<PathBuf, Error> {
    // Check the specified parent exists
    let parent = Path::new(value);
    if parent.is_dir() {
        Ok(parent.to_path_buf())
    } else {
        Err(format_err!("Invalid directory: '{}', exiting", value))
    }
}

fn get_threshold(value: &str) -> Result<Threshold, Error> {
    if value.ends_with("%") {
        // unwrap is safe
        let mut value = value.to_string();
        value.pop();
        let percent = value.parse()?;
        Ok(Threshold::Percent(percent))
    } else {
        let bytes = Byte::from_str(value).map_err(
            |e| format_err!("Invalid threshold: {}", e))?;
        Ok(Threshold::Bytes(bytes.get_bytes() as u64))
    }
}

fn main() -> Result<(), Error> {
    logger::init();
    let opts: Opts = Opts::parse();

    ReclaimLoop{
        parent: get_parent(&opts.parent)?,
        interval: opts.interval,
        cooldown: opts.cooldown,
        threshold: get_threshold(&opts.threshold)?,
    }.start();

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{get_threshold, Threshold, ReclaimLoop, ReclaimState};
    use std::collections::HashMap;
    use std::time::Instant;
    use failure::_core::time::Duration;
    use std::path::PathBuf;

    #[test]
    fn test_threshold() {
        let string = "50%";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Percent(50f64));

        let string = "100";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(100));

        let string = "100KB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(100_000));

        let string = "100KiB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(102_400));

        let string = "100MB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(100_000_000));

        let string = "100MiB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(104_857_600));

        let string = "100GB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(100_000_000_000));

        let string = "100GiB";
        let threshold = get_threshold(string).unwrap();
        assert_eq!(threshold, Threshold::Bytes(107_374_182_400));
    }

    #[test]
    fn test_reclaim_loop_cleanup() {
        let reclaim_loop = ReclaimLoop {
            parent: PathBuf::new(),
            interval: 0,
            cooldown: 0,
            threshold: Threshold::Bytes(0),
        };

        let second = Duration::from_secs(1);
        let now = Instant::now();
        let before = now - second;
        let after = now + second;
        let mut states = HashMap::new();

        states.insert(PathBuf::from("never"), ReclaimState{
            last_seen: None, last_reclaimed: None, last_error: None});
        states.insert(PathBuf::from("before"), ReclaimState{
            last_seen: Some(before), last_reclaimed: None, last_error: None});
        states.insert(PathBuf::from("after"), ReclaimState{
            last_seen: Some(after), last_reclaimed: None, last_error: None});

        reclaim_loop.cleanup(&now, &mut states);

        assert!(! states.contains_key(&PathBuf::from("never")));
        assert!(! states.contains_key(&PathBuf::from("before")));
        assert!(states.contains_key(&PathBuf::from("after")));
    }
}