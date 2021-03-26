use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::path;
use std::hash::Hasher;

use actix_files::Files;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use log::{info, warn};

use handlebars::{Handlebars, RenderError};
use sass_rs::{compile_file, Options};

use serde::Serialize;

#[macro_use]
extern crate serde_json;

#[macro_use]
extern crate lazy_static;

extern crate yaml_rust;
use yaml_rust::{Yaml, YamlEmitter, YamlLoader};

extern crate notify;
use notify::{watcher, DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::fs::File;
use std::io::prelude::*;
use std::collections::HashMap;
use std::iter::FromIterator;
// thanks rust-lang git repo for the sass compile stuff

#[derive(Clone, Serialize)]
struct CSSFiles {
    app: String,
    fonts: String,
    //vendor: String,
}
#[derive(Clone, Serialize)]
struct JSFiles {
    app: String,
}
#[derive(Clone, Serialize)]
struct AssetFiles {
    css: CSSFiles,
    //js: JSFiles,
}

struct ServerConfig {
    dev: bool,
}

lazy_static! {
    static ref ASSETS: AssetFiles = {
        let app_css_file = compile_sass("app");
        let fonts_css_file = compile_sass("fonts");
        //let vendor_css_file = concat_vendor_css(vec!["tachyons"]);
        //let app_js_file = concat_app_js(vec!["tools-install"]);

        AssetFiles {
            css: CSSFiles {
                app: app_css_file,
                fonts: fonts_css_file,
            //    vendor: vendor_css_file,
            },
            //js: JSFiles { app: app_js_file },
        }
    };

    static ref CONFIG: ServerConfig = {
        let dev = match env::var("DEV") {
            Ok(_val) => true,
            Err(_e) => false,
        };
        ServerConfig {
            dev: dev
        }
    };
}

macro_rules! compileOrFetch {
    ($data:expr, $name:literal, $assetA:tt.$assetB:tt, $compileFunc:tt) => {
        if CONFIG.dev {
            $compileFunc($name)
        } else {
            String::from(&$data.assets.$assetA.$assetB)
        }
    };
}

macro_rules! getValue {
    ($doc:ident, $key:literal, $as:tt, $default:literal) => {
        $doc[$key].$as().unwrap_or($default)
    }
}

#[derive(Serialize)]
struct ProjectFlags {
    readme_thumbnail: bool, // is thumbnail taken from the README
}

impl ProjectFlags {
    fn from(doc: &Yaml) -> ProjectFlags {
        match doc["flags"].as_str() {
            Some(flags) => {
                println!("{}", flags);
                return ProjectFlags {
                    // readme_thumbnail: getValue!(flags, "readme_thumbnail", as_bool, false),
                    readme_thumbnail: false,
                }
            },
            None => {
                return ProjectFlags {
                    readme_thumbnail: false,
                }
            }
        }
    }
}

#[derive(Serialize)]
struct Project {
    id: String,
    thumbnail: String,
    title: String,
    description: String,
    url: String,
    stars: i64,
    forks: i64,
    flags: ProjectFlags,
}

struct Projects {
    rx: Receiver<notify::DebouncedEvent>,
    watcher: RecommendedWatcher,
    value: Arc<Mutex<HashMap<String, Project>>>,
}


impl Projects {
    fn new() -> Projects {
        let (tx, rx) = channel();
        let watcher = watcher(tx, Duration::from_secs(2)).unwrap();
        return Projects {
            rx: rx,
            watcher: watcher,
            value: Arc::new(Mutex::new(HashMap::new())),
        };
    }

    fn process(&self, path: std::path::PathBuf) {
        let file = File::open(path.clone());
        if file.is_err() {
            println!("File '{}' not found!", path.to_str().unwrap());
            return;
        }
        let mut file = file.unwrap();
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        let docs = YamlLoader::load_from_str(&s);
        if docs.is_err() {
            return;
        }

        let doc = &docs.unwrap()[0];
        match doc["id"].as_str() {
            Some(id) => {
                self.value.lock().unwrap().insert(id.to_string(),
                    Project {
                        id: id.to_string(),
                        thumbnail: String::from(getValue!(doc, "thumbnail", as_str, "default")),
                        title: String::from(getValue!(doc, "title", as_str, "default")),
                        description: String::from(getValue!(doc, "description", as_str, "default")),
                        url: String::from(getValue!(doc, "url", as_str, "default")),
                        stars: getValue!(doc, "stars", as_i64, 0),
                        forks: getValue!(doc, "forks", as_i64, 0),
                        flags: ProjectFlags::from(doc)
                    },
                );
            }
            None => {
                println!(
                    "'{}' - Project ID not found!",
                    path.into_os_string()
                        .into_string()
                        .unwrap_or(String::from("???"))
                );
            }
        }
        
        println!("{:?}", doc);
    }

    fn value(&self) -> Arc<Mutex<HashMap<String, Project>>> {
        println!("fetching project values");

        self.rx.try_iter().for_each(|e: DebouncedEvent| match e {
            DebouncedEvent::NoticeWrite(p)  => self.process(p),
            DebouncedEvent::NoticeRemove(p) => self.process(p),
            DebouncedEvent::Create(p)       => self.process(p),
            DebouncedEvent::Write(p)        => self.process(p),
            DebouncedEvent::Chmod(p)        => self.process(p),
            DebouncedEvent::Remove(p)       => {
                self.value.lock().unwrap().remove(&String::from(p.file_name().unwrap().to_str().unwrap()));
            },
            DebouncedEvent::Rename(_, p)    => self.process(p),
            _ => {}
        });
        return self.value.clone();
    }
}

struct AppState<'a> {
    hb: web::Data<Handlebars<'a>>,
    projects: Arc<Mutex<Projects>>,
    assets: &'a AssetFiles,
}

#[get("/")] // TODO: actually learn about lifetime specifiers
async fn index(data: web::Data<AppState<'_>>) -> impl Responder {
    render_template(String::from("index"), data, None)
}

#[get("/projects")]
async fn get_projects(data: web::Data<AppState<'_>>) -> impl Responder {
    let proj = &data.projects.lock().unwrap().value();
    let map = proj.lock().unwrap();
    render_template(String::from("projects"), data, Some(json!({
        "projects": Vec::from_iter(map.values())
    })))
}

#[get("/projects/{id}")]
async fn get_project(
    web::Path(id): web::Path<String>,
    data: web::Data<AppState<'_>>,
) -> impl Responder {
    let proj = &data.projects.lock().unwrap().value();
    let map = proj.lock().unwrap();
    println!("project {}", id);
    render_template(String::from("project"), data, Some(json!({
        "project": map.get(&id)
    })))
}

#[get("/{page}")]
async fn get_page(
    web::Path(page): web::Path<String>,
    data: web::Data<AppState<'_>>,
) -> impl Responder {
    println!("page {}", page);
    render_template(page, data, None)
}

fn render_fail_wrapper(
    res: Result<String, RenderError>,
) -> actix_web::web::HttpResponse<actix_web::dev::Body> {
    match res {
        Ok(content) => HttpResponse::Ok().body(content),
        Err(_) => HttpResponse::Ok().body("<h1>404</h1>"),
    }
}

/*

render(data) {
    const final_data = {
        app_css:
        ...data
    }
    // do render things with final_data
}

*/

fn render_template(
    page: String,
    data: web::Data<AppState<'_>>,
    json: Option<serde_json::Value>
) -> actix_web::web::HttpResponse<actix_web::dev::Body> {
    println!("Template request for '{}'", page);
    let mut dm = json!({
        "page": page,
        "app_css": compileOrFetch! (data, "app", css.app, compile_sass),
    });

    let d = dm.as_object_mut().unwrap();

    match json {
        Some(j) => {
            j.as_object().unwrap().iter().for_each(|(k,v)| {
                d.insert(k.to_string(),v.clone());
            });
        },
        None => {}
    }

    let d = json!(d);
    
    if CONFIG.dev {
        let mut handlebars = Handlebars::new();
        handlebars
            .register_templates_directory(".handlebars", "./src/templates")
            .unwrap();
        render_fail_wrapper(handlebars.render(page.as_str(), &d))
    } else {
        render_fail_wrapper((&data.hb).render(page.as_str(), &d))
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    lazy_static::initialize(&ASSETS);
    // Handlebars uses a repository for the compiled templates. This object must be
    // shared between the application threads, and is therefore passed to the
    // Application Builder as an atomic reference-counted pointer.
    let mut handlebars = Handlebars::new();
    handlebars
        .register_templates_directory(".handlebars", "./src/templates")
        .unwrap();
    let handlebars_ref = web::Data::new(handlebars);
    let mut projects = Projects::new();
    projects
        .watcher
        .watch("./src/projects/", RecursiveMode::Recursive)
        .unwrap();

    let paths = fs::read_dir("./src/projects/").unwrap();
    for path in paths {
        projects.process(path.unwrap().path());
    }

    let projects_ref = Arc::new(Mutex::new(projects));

    println!("Webserver running!");
    HttpServer::new(move || {
        App::new()
            .data(AppState {
                hb: handlebars_ref.clone(),
                assets: &ASSETS,
                projects: projects_ref.clone(),
            })
            .service(Files::new("/static", "./static"))
            .service(index)
            .service(get_projects)
            .service(get_project)
            .service(get_page)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}

fn hash_css(css: &str) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(css.as_bytes());
    hasher.finish().to_string()
}

fn compile_sass(filename: &str) -> String {
    println!("Compiling '{}.css'...", filename);
    let scss_file = format!("./src/styles/{}.scss", filename);

    if !path::Path::new(&scss_file).exists() {
        let path = std::env::current_dir().unwrap();
        println!("The current directory is {}", path.display());
        panic!("file not found: {}", scss_file);
    }

    let css = compile_file(&scss_file, Options::default())
        .unwrap_or_else(|_| panic!("couldn't compile sass: {}", &scss_file));

    let css_sha = format!("{}_{}", filename, hash_css(&css));
    let css_file = format!("./static/styles/{}.css", css_sha);

    fs::write(&css_file, css.into_bytes())
        .unwrap_or_else(|_| panic!("couldn't write css file: {}", &css_file));

    String::from(&css_file[1..])
}
