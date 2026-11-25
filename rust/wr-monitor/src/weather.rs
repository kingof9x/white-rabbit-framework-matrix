//! The weather app's readings, as `led_mon/plugins/time_weather_plugin.py` gets them: the location
//! from Windows Location when the app asks for it (location.rs), else the app's lat_lon or zip_info,
//! otherwise the internet address; then the weather from
//! OpenWeather when OPENWEATHER_API_KEY is set, and from Open-Meteo otherwise or when that fails.
//! Asked again every 30 seconds, on its own thread.
//!
//! `sensors.rs` keeps one `background.rs` reader per distinct weather setting (`WeatherSettings::key`),
//! so two weather apps with different places each get their own thread. Everything here blocks
//! (HTTP with a 10 s time limit), which is fine on that thread. The parsing of the services' JSON
//! answers lives in `wr_core::apps::weather` so it can be tested without the network.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use wr_core::apps::weather::{parse_open_meteo, parse_openweather, Weather};
use wr_core::layout::App;

use crate::background::OnceLog;

/// How often the weather thread asks again.
pub const REFRESH: Duration = Duration::from_secs(30);

/// OpenWeather (needs an API key from the OPENWEATHER_API_KEY environment variable).
const OPENWEATHER_HOST: &str = "https://api.openweathermap.org";
/// Open-Meteo's forecast service (free, no key).
const OPEN_METEO_HOST: &str = "https://api.open-meteo.com";
/// Open-Meteo's place-name search, used to turn a zip code into coordinates without a key.
const OPEN_METEO_GEOCODE_HOST: &str = "https://geocoding-api.open-meteo.com";

/// The app's settings that change what is asked.
///
/// Built from the layout's app arguments. Two apps with equal settings share one reader.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherSettings {
    /// `zip_info`: (zip or postal code, country code), e.g. ("10001", "US").
    pub zip: Option<(String, String)>,
    /// `lat_lon`: (latitude, longitude) in degrees, typed in by the user.
    pub lat_lon: Option<(f64, f64)>,
    /// "windows" to try Windows Location first; anything else ("manual", the default) skips it.
    pub location_source: String,
    /// "metric" (°C, km/h), "imperial" (°F, mph) or "standard" (OpenWeather's kelvin).
    pub units: String,
    /// Show a forecast instead of the current weather.
    pub forecast: bool,
    /// Days ahead for the forecast (1 = tomorrow); negative counts as 0 (today).
    pub forecast_day: i64,
    /// Hour of the forecast day (0-23).
    pub forecast_hour: i64,
}

/// A YAML value as a number, accepting both `51.5` and `"51.5"` since layouts are hand-edited.
fn yaml_f64(v: &serde_yaml::Value) -> Option<f64> {
    match v {
        serde_yaml::Value::Number(n) => n.as_f64(),
        serde_yaml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A YAML value as text; numbers are accepted (a zip code like 10001 is read by YAML as a number).
fn yaml_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

impl WeatherSettings {
    /// Reads the settings from one weather app of the layout, with defaults for anything missing.
    /// Unknown units are logged and replaced with metric; a malformed pair is ignored (None).
    pub fn of(app: &App) -> WeatherSettings {
        let pair = |key: &str| app.arg(key).and_then(|v| v.as_sequence()).filter(|s| s.len() == 2).cloned();
        let int = |key: &str, default: i64| app.arg(key).and_then(yaml_f64).map(|v| v as i64).unwrap_or(default);
        let mut units = app.arg_str("units").unwrap_or_else(|| "metric".into());
        if !["metric", "imperial", "standard"].contains(&units.as_str()) {
            wr_core::warn!("Unrecognized weather units '{units}', using metric");
            units = "metric".into();
        }
        WeatherSettings {
            zip: pair("zip_info").map(|p| (yaml_string(&p[0]), yaml_string(&p[1]))),
            lat_lon: pair("lat_lon").and_then(|p| Some((yaml_f64(&p[0])?, yaml_f64(&p[1])?))),
            location_source: app.arg_str("location_source").unwrap_or_else(|| "manual".into()),
            units,
            forecast: app.arg_bool("forecast"),
            forecast_day: int("forecast_day", 1),
            forecast_hour: int("forecast_hour", 12),
        }
    }

    /// One string per distinct setting, to keep one reader per setting.
    ///
    /// The `Debug` text of the whole struct serves as the key: equal settings give equal text.
    pub fn key(&self) -> String {
        format!("{self:?}")
    }
}

/// Locations found once are kept: a zip code's or this computer's place doesn't move.
///
/// Keys are "zip:(code, country)" or "ip". Shared by all weather threads; lock errors are ignored
/// (the lookup is then just done again).
static LOCATIONS: Mutex<Option<HashMap<String, (f64, f64)>>> = Mutex::new(None);

/// A location found before under this key, if any.
fn remembered(key: &str) -> Option<(f64, f64)> {
    LOCATIONS.lock().ok()?.as_ref()?.get(key).copied()
}

/// Keeps a found location for the rest of the run.
fn remember(key: &str, loc: (f64, f64)) {
    if let Ok(mut map) = LOCATIONS.lock() {
        map.get_or_insert_with(HashMap::new).insert(key.to_string(), loc);
    }
}

/// An HTTP client with its connection pool, kept for the life of one weather reader.
struct Client {
    agent: ureq::Agent,
}

impl Client {
    /// Every request gives up after 10 s in total. Windows' own TLS (NativeTls) is used, so the
    /// system's certificates (and any company proxy certificates) are trusted.
    fn new() -> Client {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
        let config =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).tls_config(tls).build();
        Client { agent: config.into() }
    }

    /// GETs `url` with the query parameters (URL-encoded by ureq) and parses the answer as JSON.
    /// Err holds a readable message for network errors or an answer that isn't JSON.
    fn json(&self, url: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let mut request = self.agent.get(url);
        for (k, v) in query {
            request = request.query(*k, v);
        }
        // Error answers (a bad key) still carry JSON that says why
        let mut response = request.config().http_status_as_error(false).build().call().map_err(|e| e.to_string())?;
        response.body_mut().read_json::<Value>().map_err(|e| e.to_string())
    }

    /// The coordinates of a zip code from OpenWeather's geocoding service.
    fn zip_openweather(&self, zip: &(String, String), key: &str) -> Result<(f64, f64), String> {
        let data = self.json(
            &format!("{OPENWEATHER_HOST}/geo/1.0/zip"),
            &[("zip", format!("{},{}", zip.0, zip.1)), ("appid", key.to_string())],
        )?;
        match (data.get("lat").and_then(Value::as_f64), data.get("lon").and_then(Value::as_f64)) {
            (Some(lat), Some(lon)) => Ok((lat, lon)),
            _ => Err(format!("OpenWeather zip lookup failed: {}", data.get("message").unwrap_or(&data))),
        }
    }

    /// The coordinates of a zip code from Open-Meteo's place search, taking the first match.
    fn zip_open_meteo(&self, zip: &(String, String)) -> Result<(f64, f64), String> {
        let url = format!("{OPEN_METEO_GEOCODE_HOST}/v1/search");
        let country = zip.1.to_uppercase();
        let mut query = vec![
            ("name", zip.0.clone()),
            ("count", "1".to_string()),
            ("language", "en".to_string()),
            ("format", "json".to_string()),
        ];
        if !country.is_empty() {
            query.push(("countryCode", country.clone()));
        }
        let mut data = self.json(&url, &query)?;
        let empty = |d: &Value| d.get("results").and_then(Value::as_array).is_none_or(|r| r.is_empty());
        // Nothing found with the country filter: search again for "code COUNTRY" as plain text,
        // without the filter (`pop` removes the countryCode parameter, which was added last).
        if empty(&data) && !country.is_empty() {
            query[0].1 = format!("{} {country}", zip.0);
            query.pop();
            data = self.json(&url, &query)?;
        }
        let first = data.get("results").and_then(|r| r.get(0));
        match first.map(|f| (f.get("latitude").and_then(Value::as_f64), f.get("longitude").and_then(Value::as_f64))) {
            Some((Some(lat), Some(lon))) => Ok((lat, lon)),
            _ => Err(format!("Open-Meteo geocoding lookup failed: {}", data.get("reason").unwrap_or(&data))),
        }
    }

    /// This computer's place from its internet address, from the first of three services that answers.
    ///
    /// Tries ipwho.is, ipapi.co, then ipinfo.io; the error lists why each one failed.
    fn by_ip(&self) -> Result<(f64, f64), String> {
        let mut errors = Vec::new();
        // ipwho.is answers 200 even on failure, with `"success": false`.
        match self.json("https://ipwho.is", &[]) {
            Ok(d) if d.get("success").and_then(Value::as_bool) == Some(true) => {
                if let (Some(lat), Some(lon)) = (d["latitude"].as_f64(), d["longitude"].as_f64()) {
                    return Ok((lat, lon));
                }
                errors.push(format!("ipwho.is: {d}"));
            }
            Ok(d) => errors.push(format!("ipwho.is: {d}")),
            Err(e) => errors.push(format!("ipwho.is: {e}")),
        }
        match self.json("https://ipapi.co/json", &[]) {
            Ok(d) => {
                if let (Some(lat), Some(lon)) = (d["latitude"].as_f64(), d["longitude"].as_f64()) {
                    return Ok((lat, lon));
                }
                errors.push(format!("ipapi.co: {d}"));
            }
            Err(e) => errors.push(format!("ipapi.co: {e}")),
        }
        match self.json("https://ipinfo.io/json", &[]) {
            Ok(d) => {
                // ipinfo.io gives the place as one string, "lat,lon".
                let loc = d["loc"].as_str().and_then(|s| s.split_once(','));
                if let Some((Ok(lat), Ok(lon))) = loc.map(|(a, b)| (a.trim().parse(), b.trim().parse())) {
                    return Ok((lat, lon));
                }
                errors.push(format!("ipinfo.io: {d}"));
            }
            Err(e) => errors.push(format!("ipinfo.io: {e}")),
        }
        Err(format!("No location set and finding it from the internet address failed ({})", errors.join("; ")))
    }

    /// Where to get the weather for, in this order: Windows Location (if chosen and it works), the
    /// typed-in coordinates, the zip code (OpenWeather if there is a key, else or then Open-Meteo),
    /// and last the internet address. Zip and internet-address results are remembered.
    fn location(&self, s: &WeatherSettings, api_key: Option<&str>) -> Result<(f64, f64), String> {
        if s.location_source == "windows" {
            // When Windows can't say (logged once by location.rs), the typed-in location or the
            // internet address is used instead
            if let Ok(loc) = crate::location::position() {
                return Ok(loc);
            }
        }
        if let Some(loc) = s.lat_lon {
            return Ok(loc);
        }
        if let Some(zip) = &s.zip {
            let key = format!("zip:{zip:?}");
            if let Some(loc) = remembered(&key) {
                return Ok(loc);
            }
            let mut found = Err(String::new());
            if let Some(api_key) = api_key {
                found = self.zip_openweather(zip, api_key);
                if let Err(e) = &found {
                    wr_core::warn!("OpenWeather zip lookup failed; trying Open-Meteo: {e}");
                }
            }
            if found.is_err() {
                found = self.zip_open_meteo(zip);
            }
            let loc = found?;
            remember(&key, loc);
            return Ok(loc);
        }
        if let Some(loc) = remembered("ip") {
            return Ok(loc);
        }
        let loc = self.by_ip()?;
        remember("ip", loc);
        Ok(loc)
    }

    /// The weather for these settings: OpenWeather first when a key is set, then Open-Meteo.
    /// Err joins the reasons both failed. A failing OpenWeather is logged on every call.
    fn weather(&self, s: &WeatherSettings) -> Result<Weather, String> {
        let api_key = std::env::var("OPENWEATHER_API_KEY").ok().filter(|k| !k.trim().is_empty());
        let loc = self.location(s, api_key.as_deref())?;
        // The forecast's date in UTC, matching Open-Meteo's `timezone=GMT` below.
        let date = (chrono::Utc::now().date_naive() + chrono::Days::new(s.forecast_day.max(0) as u64))
            .format("%Y-%m-%d")
            .to_string();
        // None for the current weather; Some((date, hour)) picks one entry from the forecast list.
        let forecast = s.forecast.then_some((date.as_str(), s.forecast_hour));
        let mut errors = Vec::new();
        if let Some(key) = &api_key {
            // OpenWeather's free 5-day/3-hour forecast, or the current weather.
            let path = if s.forecast { "forecast" } else { "weather" };
            let answer = self
                .json(
                    &format!("{OPENWEATHER_HOST}/data/2.5/{path}"),
                    &[
                        ("lat", loc.0.to_string()),
                        ("lon", loc.1.to_string()),
                        ("appid", key.clone()),
                        ("units", s.units.clone()),
                    ],
                )
                .and_then(|d| parse_openweather(&d, &s.units, forecast));
            match answer {
                Ok(w) => return Ok(w),
                Err(e) => {
                    wr_core::warn!("OpenWeather weather lookup failed; trying Open-Meteo: {e}");
                    errors.push(format!("OpenWeather: {e}"));
                }
            }
        }
        // The values the weather app draws; asked for hour by hour for a forecast, else for now.
        let fields = "temperature_2m,apparent_temperature,wind_speed_10m,wind_direction_10m,weather_code";
        let imperial = s.units == "imperial";
        let query = [
            ("latitude", loc.0.to_string()),
            ("longitude", loc.1.to_string()),
            ("temperature_unit", if imperial { "fahrenheit" } else { "celsius" }.to_string()),
            ("wind_speed_unit", if imperial { "mph" } else { "kmh" }.to_string()),
            ("timezone", "GMT".to_string()),
            (if s.forecast { "hourly" } else { "current" }, fields.to_string()),
        ];
        match self
            .json(&format!("{OPEN_METEO_HOST}/v1/forecast"), &query)
            .and_then(|d| parse_open_meteo(&d, &s.units, forecast))
        {
            Ok(w) => Ok(w),
            Err(e) => {
                errors.push(format!("Open-Meteo: {e}"));
                Err(errors.join(" | "))
            }
        }
    }
}

/// Reads the weather for one setting. Made and called on that setting's weather thread.
///
/// Each call blocks for up to a few web requests. None when the weather can't be had; the failure
/// is logged once (until it changes or works again).
pub fn reader(settings: WeatherSettings) -> Box<dyn FnMut() -> Option<Weather>> {
    let client = Client::new();
    let mut log = OnceLog::default();
    Box::new(move || match client.weather(&settings) {
        Ok(w) => {
            log.ok("Getting the weather");
            Some(w)
        }
        Err(e) => {
            log.fail(format!("Error getting weather: {e}"));
            None
        }
    })
}
