//! The weather app's readings, as `led_mon/plugins/time_weather_plugin.py` gets them: the location
//! from the app's lat_lon or zip_info, otherwise from the internet address; then the weather from
//! OpenWeather when OPENWEATHER_API_KEY is set, and from Open-Meteo otherwise or when that fails.
//! Asked again every 30 seconds, on its own thread.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use wr_core::apps::weather::{parse_open_meteo, parse_openweather, Weather};
use wr_core::layout::App;

use crate::background::OnceLog;

pub const REFRESH: Duration = Duration::from_secs(30);

const OPENWEATHER_HOST: &str = "https://api.openweathermap.org";
const OPEN_METEO_HOST: &str = "https://api.open-meteo.com";
const OPEN_METEO_GEOCODE_HOST: &str = "https://geocoding-api.open-meteo.com";

/// The app's settings that change what is asked.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherSettings {
    pub zip: Option<(String, String)>,
    pub lat_lon: Option<(f64, f64)>,
    pub location_source: String,
    pub units: String,
    pub forecast: bool,
    pub forecast_day: i64,
    pub forecast_hour: i64,
}

fn yaml_f64(v: &serde_yaml::Value) -> Option<f64> {
    match v {
        serde_yaml::Value::Number(n) => n.as_f64(),
        serde_yaml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn yaml_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

impl WeatherSettings {
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
    pub fn key(&self) -> String {
        format!("{self:?}")
    }
}

/// Locations found once are kept: a zip code's or this computer's place doesn't move.
static LOCATIONS: Mutex<Option<HashMap<String, (f64, f64)>>> = Mutex::new(None);

fn remembered(key: &str) -> Option<(f64, f64)> {
    LOCATIONS.lock().ok()?.as_ref()?.get(key).copied()
}

fn remember(key: &str, loc: (f64, f64)) {
    if let Ok(mut map) = LOCATIONS.lock() {
        map.get_or_insert_with(HashMap::new).insert(key.to_string(), loc);
    }
}

struct Client {
    agent: ureq::Agent,
}

impl Client {
    fn new() -> Client {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
        let config =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).tls_config(tls).build();
        Client { agent: config.into() }
    }

    fn json(&self, url: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let mut request = self.agent.get(url);
        for (k, v) in query {
            request = request.query(*k, v);
        }
        // Error answers (a bad key) still carry JSON that says why
        let mut response = request.config().http_status_as_error(false).build().call().map_err(|e| e.to_string())?;
        response.body_mut().read_json::<Value>().map_err(|e| e.to_string())
    }

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
    fn by_ip(&self) -> Result<(f64, f64), String> {
        let mut errors = Vec::new();
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

    fn location(&self, s: &WeatherSettings, api_key: Option<&str>) -> Result<(f64, f64), String> {
        // Windows' own location ("location_source: windows") isn't read here yet; the typed-in
        // location or the internet address is used instead, as Python does when it is unavailable.
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

    fn weather(&self, s: &WeatherSettings) -> Result<Weather, String> {
        let api_key = std::env::var("OPENWEATHER_API_KEY").ok().filter(|k| !k.trim().is_empty());
        let loc = self.location(s, api_key.as_deref())?;
        let date = (chrono::Utc::now().date_naive() + chrono::Days::new(s.forecast_day.max(0) as u64))
            .format("%Y-%m-%d")
            .to_string();
        let forecast = s.forecast.then_some((date.as_str(), s.forecast_hour));
        let mut errors = Vec::new();
        if let Some(key) = &api_key {
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
