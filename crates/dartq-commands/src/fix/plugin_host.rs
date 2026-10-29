use super::*;

/// One entry of the governing options file's `plugins:` section: the
/// declared package name plus the source the value pins. Upstream
/// `_parsePluginSource` (analysis_options_parse_model.dart:1639) resolves
/// the entry's source kind in a fixed order — a scalar is a version
/// constraint, a mapping prefers `version`/`hosted`, then `git`, then
/// `path`, first match wins — and only `path` carries a filesystem location
/// dartq resolves itself: a `path:`-valued declaration names the plugin
/// package's directory directly (relative to the options file's parent,
/// normalized — the parity corpora's own layout), while `version`/`hosted`/
/// `git` resolve through pub's `package_config.json` like a bare scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredPlugin {
    /// The package name (the section key).
    pub name: String,
    /// The `path:` source's package directory — absolute already, or
    /// resolved against the options file's parent and lexically normalized
    /// (upstream `pathContext.join` + `normalize`, _parsePluginSource).
    /// `None` for every other source kind — pub resolves those.
    pub path: Option<PathBuf>,
}

/// The `path:` source of one plugin mapping value, when it selects `path`:
/// upstream checks `version`/`hosted`/`git` first — a mapping carrying any
/// of them never reaches the `path` arm (first match wins; the options
/// validator reports the surplus). A `path` scalar resolves against the
/// options file's parent and normalizes (upstream's `_parsePluginSource`
/// `join`/`normalize`); a non-string `path` is no source at all.
pub(crate) fn declared_plugin_path(value: &Yaml, options_dir: &Path) -> Option<PathBuf> {
    let Yaml::Mapping(mapping) = value else {
        return None;
    };
    // The upstream arm order: `version`/`hosted` and `git` shadow `path`
    // (first match wins) — only their absence lets `path` select.
    if mapping.contains_key("version")
        || mapping.contains_key("hosted")
        || mapping.contains_key("git")
    {
        return None;
    }
    let Yaml::String(path) = mapping.get("path")? else {
        return None;
    };
    let path = PathBuf::from(path);
    Some(if path.is_relative() {
        dartq_pub::normalize(&options_dir.join(path))
    } else {
        dartq_pub::normalize(&path)
    })
}

/// The governing root options file of `workspace_root` (the engine's
/// locator) and the plugin packages its top-level `plugins:` section
/// declares, in declaration order — or `None` when the workspace declares
/// nothing: no options file, unreadable, malformed YAML, no `plugins:`
/// section, or a section whose keys name no plugin (only
/// `dependency_overrides` — a hosting face, never a plugin entry — or keys
/// that are not strings). This is the single discovery face of the
/// section — pub-native plugin packages, resolved by pub into
/// `package_config.json` — shared by the fix/assist dispatch here and the
/// daemon's session launch; rule configuration is a separate face read
/// elsewhere: a bare version-constraint entry (`my_plugin: ^1.0.0`)
/// declares a plugin without configuring rules.
///
/// The `None` shape is the absent path of the core-first invariant: it
/// allocates nothing (no path clone, no vec), so a plugin-free
/// workspace's only launch cost is the locator walk plus this one
/// read-and-parse, the minimum needed to establish absence. A broken
/// declaration reports through the engine's options face and never takes
/// the launch down.
pub fn declared_plugins(workspace_root: &Path) -> Option<(PathBuf, Vec<String>)> {
    let (file, declared) = declared_plugin_sources(workspace_root)?;
    Some((file, declared.into_iter().map(|decl| decl.name).collect()))
}

/// [`declared_plugins`] with each declaration's resolved `path:` source —
/// the launch faces' full form: `path`-valued entries resolve against that
/// directory directly (pub never sees them), while every other source
/// stays pub-resolved through `package_config.json`. The name projection
/// ([`declared_plugins`]) is what the pub-native discovery face consumes;
/// the richer face is where the `path:` entries augment the config the
/// discovery resolves through.
pub fn declared_plugin_sources(workspace_root: &Path) -> Option<(PathBuf, Vec<DeclaredPlugin>)> {
    let file = AnalysisOptions::find_for_dir(workspace_root)?;
    let text = std::fs::read_to_string(&file).ok()?;
    let Ok(Yaml::Mapping(root)) = serde_yaml::from_str::<Yaml>(&text) else {
        return None;
    };
    let Some(Yaml::Mapping(plugins)) = root.get("plugins") else {
        return None;
    };
    let options_dir = file.parent().unwrap_or(Path::new(""));
    let declared: Vec<DeclaredPlugin> = plugins
        .iter()
        .filter_map(|(key, value)| {
            let name = key.as_str()?;
            // The upstream key match is exact — `dependencyOverrides` on a
            // differently-cased spelling is a plugin name, not the
            // overrides section (parse_model:1561 `pluginName ==
            // AnalysisOptionsFileKeys.dependencyOverrides`).
            if name == "dependency_overrides" {
                return None;
            }
            Some(DeclaredPlugin {
                name: name.to_owned(),
                path: declared_plugin_path(value, options_dir),
            })
        })
        .collect();
    // A section that names no plugin (only `dependency_overrides`, or keys
    // that are not strings) is the absent shape.
    if declared.is_empty() {
        None
    } else {
        Some((file, declared))
    }
}

/// The `path:`-augmented package config the pub-native discovery resolves
/// through: one synthetic entry per `path:`-declared name absent from the
/// real `package_config.json` (a pub-resolved entry always wins — pub's
/// resolution is the artifact of record), so `discover`'s unchanged
/// pub-native arm resolves the declaration to its directory. The synthetic
/// entry's `root_uri` is the `file:` URI of the (absolute or
/// options-file-anchored) plugin directory; `package_uri` is `lib/` — the
/// derived-manifest convention (`lib/main.dart`) then applies like any
/// pub-native hit. Only called when `path:` declarations exist — the
/// plugin-free workspace never reaches it.
pub fn plugin_package_config(
    pub_config: &PackageConfig,
    declared: &[DeclaredPlugin],
) -> PackageConfig {
    let mut config = pub_config.clone();
    for decl in declared {
        let Some(path) = &decl.path else {
            continue;
        };
        if config.entry(&decl.name).is_some() {
            continue;
        }
        config.packages.push(PackageConfigEntry {
            name: decl.name.clone(),
            root_uri: format!("file://{}", path.display()),
            package_uri: Some("lib/".to_owned()),
            language_version: None,
            additional_properties: BTreeMap::new(),
        });
    }
    config
}

/// The plugin dispatch face: every plugin discovered under
/// `<workspace_root>/plugins/` with a live runtime, answering
/// `request/fixes` on the fix and assist faces' behalf.
///
/// [`PluginHosts::launch`] spawns eagerly and completes each handshake
/// before returning — the caller pays up to the manifest's
/// `request_ms × (retries + 1)` per plugin (defaults 10 s × 4). Spawn
/// failures log and surface once through [`PluginHosts::take_errors`];
/// they never fail the face. Dropping the set without
/// [`PluginHosts::shutdown`] kills the children without the graceful
/// handshake.
pub struct PluginHosts {
    pub(crate) hosts: Vec<Host>,
}

/// One plugin's dispatch state. `runtime: None` marks the plugin dead for
/// this run: the first surfaced runtime error means the runtime already
/// spent its restart budget reaching it, and further requests would burn
/// the manifest's budget on a plugin that cannot come back.
pub(crate) struct Host {
    pub(crate) name: String,
    runtime: Option<PluginRuntime>,
    pub(crate) error: Option<String>,
}

impl PluginHosts {
    /// Discovers plugins from the explicit sources — the `<root>/plugins/`
    /// convention and the governing `analysis_options`' pub-native `plugins:`
    /// section resolved through `pub_config` (the parsed
    /// `package_config.json` in `config_dir`; no overrides) — and spawns
    /// each hit. The daemon passes its workspace root plus the resolution
    /// its engine runs on, the CLI its process working directory.
    #[must_use]
    pub fn launch(root: &Path, pub_config: &PackageConfig, config_dir: &Path) -> Self {
        let declared = declared_plugin_sources(root);
        // The absent shape: an empty path borrow and an empty slice — the
        // pub-native source contributes nothing and allocates nothing.
        let (options_file, names, decls): (&Path, Vec<String>, &[DeclaredPlugin]) =
            match declared.as_ref() {
                Some((file, decls)) => (
                    file.as_path(),
                    decls.iter().map(|decl| decl.name.clone()).collect(),
                    decls.as_slice(),
                ),
                None => (Path::new(""), Vec::new(), &[]),
            };
        // `path:`-declared names absent from pub's resolution get a
        // synthetic config entry — the unchanged discovery arm then resolves
        // them like any pub-native hit. The clone costs only when `path:`
        // declarations exist (the plugin-free workspace never reaches it).
        let augmented;
        let config = if decls.iter().any(|decl| decl.path.is_some()) {
            augmented = plugin_package_config(pub_config, decls);
            &augmented
        } else {
            pub_config
        };
        let report = discover_plugins(
            root,
            &[],
            &PubNative {
                declared: &names,
                config,
                config_dir,
                options_file,
            },
        );
        for issue in &report.skipped {
            eprintln!(
                "dartq: skipping plugin manifest `{}`: {}",
                issue.path.display(),
                issue.error
            );
        }
        let hosts = report
            .plugins
            .into_iter()
            .map(|plugin| {
                let name = plugin.manifest.name.clone();
                match PluginRuntime::spawn(SpawnSpec::dart(&plugin.dir, plugin.manifest)) {
                    Ok(runtime) => Host {
                        name,
                        runtime: Some(runtime),
                        error: None,
                    },
                    Err(error) => {
                        eprintln!("dartq: plugin `{name}` failed to launch: {error}");
                        Host {
                            name,
                            runtime: None,
                            error: Some(error.to_string()),
                        }
                    }
                }
            })
            .collect();
        Self { hosts }
    }

    /// A host set with no plugins: the no-dispatch posture (tests, and
    /// callers that resolved their plugin posture before reaching here).
    #[must_use]
    pub fn empty() -> Self {
        Self { hosts: Vec::new() }
    }

    /// True when no plugin was discovered — the faces skip their dispatch
    /// entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Asks every live host for fixes at the UTF-16 span
    /// `[offset, offset + length)` of `file`, converting each reply to
    /// [`PluginFix`] rows anchored against `text` (see [`from_plugin`]).
    /// Protocol v1 carries the assists work on the same request; a host
    /// whose negotiated capability set lacks `fix` cannot serve it and is
    /// skipped. A host's first surfaced runtime error ends its session
    /// (the isolation contract) and surfaces through
    /// [`PluginHosts::take_errors`].
    pub fn fixes(
        &mut self,
        file: &str,
        offset: u64,
        length: u64,
        text: &str,
        lines: &LineIndex,
    ) -> Vec<PluginFix> {
        let mut rows = Vec::new();
        for host in &mut self.hosts {
            let Some(runtime) = host.runtime.as_mut() else {
                continue;
            };
            if !runtime
                .negotiated()
                .is_some_and(|caps| caps.contains(Capability::Fix))
            {
                continue;
            }
            match runtime.request_fixes(file, offset, length) {
                Ok(reply) => rows.extend(from_plugin(&host.name, &reply, text, lines)),
                Err(error) => {
                    host.runtime = None;
                    host.error = Some(error.to_string());
                }
            }
        }
        rows
    }

    /// Drains the hosts' terminal failures recorded so far (spawn
    /// refusals and first runtime errors), one per plugin.
    pub fn take_errors(&mut self) -> Vec<PluginHostError> {
        self.hosts
            .iter_mut()
            .filter_map(|host| {
                host.error.take().map(|message| PluginHostError {
                    plugin: host.name.clone(),
                    message,
                })
            })
            .collect()
    }

    /// The graceful teardown (`shutdown` per live runtime). Errors log;
    /// teardown quality does not affect the run's outcome.
    pub fn shutdown(self) {
        for host in self.hosts {
            if let Some(runtime) = host.runtime
                && let Err(error) = runtime.shutdown()
            {
                eprintln!("dartq: plugin `{}` shutdown failed: {error}", host.name);
            }
        }
    }
}
