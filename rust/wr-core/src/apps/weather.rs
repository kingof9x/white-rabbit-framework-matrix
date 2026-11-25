//! The weather app (`led_mon/plugins/time_weather_plugin.py`): what it shows for a reading, and
//! reading the answers of the two weather services it asks (Open-Meteo, and OpenWeather when an
//! API key is set). Asking them is in `wr-monitor`.
//!
//! Where this sits: `wr-monitor`'s `weather.rs` fetches the weather over the internet on a background
//! thread (every few minutes) and turns the JSON answer into a `Weather` with `parse_open_meteo` or
//! `parse_openweather`. When a panel is drawn, `render::draw_data_app` (on the monitor's main loop)
//! picks which measure to show now with `measure_at`, turns it into a column of character names with
//! `weather_values`, draws them with `drawing::draw_chars_list`, and, for a forecast, adds the day and
//! hour bars with `draw_forecast_period`. The character names ("2", "degC", "clouds", "wind-sw", ...)
//! are glyphs in the pattern tables in `patterns.rs`.
//!
//! No network, file or thread code is here, so all of it is tested with fixed JSON.

use crate::pyround::int_round;
use crate::Grid;
use serde_json::Value;

/// One weather reading, in the units picked for the app.
/// Temperatures are in the picked unit already (Celsius, Fahrenheit or Kelvin), and wind speed in
/// km/h or mph, so the drawing code only has to round and print.
#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    /// The air temperature, in the unit named by `temp_symbol`.
    pub temp: f64,
    /// The "feels like" temperature (the services' apparent temperature), in the same unit as `temp`.
    pub wind_chill: f64,
    /// Wind speed in km/h (metric and standard units) or mph (imperial), as named by `wind_speed_symbol`.
    pub wind_speed: f64,
    /// "km" or "mi"
    pub wind_speed_symbol: String,
    /// Degrees, 0 is north.
    pub wind_dir: f64,
    /// "degC", "degF" or "degK"
    pub temp_symbol: String,
    /// An icon name: clear, clouds, rain, snow, mist-like, ...
    pub condition: String,
}

/// The measures the app knows: temperature plus condition icon, the "feels like" temperature, and
/// wind speed plus direction. A layout lists the ones to show in its "measures" setting.
pub const MEASURES: [&str; 3] = ["temp_condition", "wind_chill", "wind"];
/// OpenWeather's condition names that all share one "mist-like" icon (the panel has no room for a
/// separate picture of each).
const MIST_LIKE: [&str; 9] = ["Mist", "Fog", "Dust", "Haze", "Smoke", "Squall", "Ash", "Sand", "Tornado"];

/// Python's str(round(x)) as single characters: "-", "4".
/// The number is rounded the way Python rounds (halves go to the even number, so 12.5 becomes 12)
/// so the panel shows the same as the Python version did.
fn digits(x: f64) -> Vec<String> {
    int_round(x).to_string().chars().map(|c| c.to_string()).collect()
}

/// The characters the app draws for one measure, one under the other.
/// Each string is the name of a glyph drawn one below the other. An unknown measure gives "?", "?".
pub fn weather_values(w: &Weather, measure: &str) -> Vec<String> {
    match measure {
        "temp_condition" => {
            let mut v = digits(w.temp);
            v.push(w.temp_symbol.clone());
            v.push(w.condition.to_lowercase());
            v
        }
        // "wc" is a small "wind chill" label glyph; the " " leaves one empty glyph slot under it.
        "wind_chill" => {
            let mut v = vec!["wc".to_string(), " ".to_string()];
            v.extend(digits(w.wind_chill));
            v.push(w.temp_symbol.clone());
            v
        }
        "wind" => {
            const DIRS: [&str; 8] = ["n", "ne", "e", "se", "s", "sw", "w", "nw"];
            // 8 compass points, 45 degrees apart: dividing by 45 and rounding gives the nearest
            // point; rem_euclid wraps 360 (index 8) back to north and keeps any negative value in range.
            let ix = int_round(w.wind_dir / (360.0 / 8.0)).rem_euclid(8) as usize;
            let mut v = vec!["wd".to_string()];
            v.extend(digits(w.wind_speed));
            v.push(w.wind_speed_symbol.clone());
            v.push(format!("wind-{}", DIRS[ix]));
            v
        }
        _ => vec!["?".into(), "?".into()],
    }
}

/// The measure shown at `now` seconds: the app's measures take turns, `duration` seconds each.
/// Unknown measures are dropped; with none left it shows the temperature.
/// `now` is seconds on any steady clock; only its division by `duration` matters, so all panels
/// showing the app switch together. A duration of 0 or less shows only the first measure.
pub fn measure_at<'a>(measures: &[&'a str], duration: f64, now: f64) -> &'a str {
    let valid: Vec<&str> = measures.iter().copied().filter(|m| MEASURES.contains(m)).collect();
    if valid.is_empty() {
        return "temp_condition";
    }
    if duration <= 0.0 {
        return valid[0];
    }
    // Which `duration`-long slot `now` falls in, wrapped around the list.
    valid[((now / duration).floor().max(0.0) as usize) % valid.len()]
}

/// Bars up the bottom of the left and right edges for the forecast's day (1-5, left) and hour
/// (right, one row per 3 hours), with a mark beside the fourth row of the hour bar.
/// Only drawn when the app shows a forecast instead of the current weather, so you can tell the two
/// apart. The day bar fills column 0 from the bottom, one row per day ahead (1 = tomorrow, max 5);
/// the hour bar fills column 8 from the bottom, one row per 3-hour block that has started by `hour`
/// (hour 0 = 1 row, hour 21 = 8 rows). Out-of-range days are clamped to 1-5.
pub fn draw_forecast_period(grid: &mut Grid, fill_value: i32, day: i64, hour: i64) {
    let day = day.clamp(1, 5);
    // Find the last 3-hour block whose start is at or before `hour`. The starting value 8 is only
    // kept for a negative hour, which then draws 9 rows (same as Python; layouts never give that).
    let mut hours_idx = 8;
    for (idx, h) in [0, 3, 6, 9, 12, 15, 18, 21].iter().enumerate() {
        if hour >= *h {
            hours_idx = idx;
        }
    }
    let hours_idx = hours_idx as i64 + 1;
    // Row 33 is the bottom row; fill_y's end is exclusive, so these fill the bottom `day` rows
    // of column 0 and the bottom `hours_idx` rows of column 8.
    grid.fill_y(0, 33 - day + 1, 34, fill_value);
    grid.fill_y(8, 33 - hours_idx + 1, 34, fill_value);
    // The mark at noon: column 7 of row 30 (the hour bar's 4th row, 09:00-12:00) copies whatever the
    // hour bar has there, so it lights only once the bar has reached that row.
    grid.set(7, 30, grid.get(8, 30));
}

/// The temperature glyph for the app's "units" setting: "metric" is Celsius, "imperial" Fahrenheit,
/// and anything else ("standard", the services' name for it) Kelvin.
pub fn temp_symbol(units: &str) -> &'static str {
    match units {
        "metric" => "degC",
        "imperial" => "degF",
        _ => "degK",
    }
}

/// The wind speed glyph for the units: miles for imperial, kilometres otherwise (Kelvin users get km/h).
fn wind_symbol(units: &str) -> &'static str {
    if units == "imperial" {
        "mi"
    } else {
        "km"
    }
}

/// Open-Meteo's WMO weather code as an icon name.
/// WMO codes are an international table: 0 clear sky, 1-3 partly cloudy to overcast, 45/48 fog,
/// 51-57 drizzle, 61-67 and 80-82 rain (and showers), 71-77 and 85/86 snow, 95-99 thunderstorms.
/// Anything else, or no code at all, shows as clouds.
pub fn open_meteo_condition(code: Option<f64>) -> &'static str {
    let Some(code) = code else { return "clouds" };
    match code.trunc() as i64 {
        0 => "clear",
        1..=3 => "clouds",
        45 | 48 => "mist-like",
        51 | 53 | 55 | 56 | 57 => "drizzle",
        61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 => "rain",
        71 | 73 | 75 | 77 | 85 | 86 => "snow",
        95 | 96 | 99 => "thunderstorm",
        _ => "clouds",
    }
}

/// A JSON number as f64; None when the value is missing or not a number.
fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

/// "YYYY-MM-DD" and the hour of an ISO time such as "2026-11-14T15:00" or "2026-11-14 15:00:00".
/// Works on bytes 0-9 and 11-12, so both the "T" and the space form work. None when the text is
/// too short or the hour is not a number; never panics, because `get` returns None rather than
/// slicing in the middle of a multi-byte character.
fn date_and_hour(text: &str) -> Option<(&str, i64)> {
    let date = text.get(..10)?;
    let hour = text.get(11..13)?.parse().ok()?;
    Some((date, hour))
}

/// Open-Meteo's answer. `forecast_date` (YYYY-MM-DD, GMT) and `forecast_hour` pick the forecast hour:
/// the first one on that day at or after that hour, otherwise the last one given.
/// `units` is the app's setting. The request (in `wr-monitor`) already asks for Celsius or Fahrenheit
/// and km/h or mph, so only Kelvin ("standard") needs converting here. Errors are plain messages for
/// the log: the service's own error answer, or a missing field. With no forecast hour matching (a date
/// past the end of the forecast), the last hour given is used instead of failing.
pub fn parse_open_meteo(data: &Value, units: &str, forecast: Option<(&str, i64)>) -> Result<Weather, String> {
    if data.get("error").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("Open-Meteo lookup failed: {}", data.get("reason").unwrap_or(data)));
    }
    // Open-Meteo has no Kelvin option, so Kelvin is Celsius + 273.15.
    let standard = |v: f64| if units == "standard" { v + 273.15 } else { v };
    let (temp, feels, speed, dir, code) = match forecast {
        Some((date, hour)) => {
            let hourly = data.get("hourly").ok_or("Open-Meteo forecast has no hourly data")?;
            let times = hourly.get("time").and_then(Value::as_array).filter(|t| !t.is_empty());
            let times = times.ok_or("Open-Meteo forecast response did not include hourly time data.")?;
            // The first hourly entry on the wanted date at or after the wanted hour.
            let idx = times
                .iter()
                .position(|t| t.as_str().and_then(date_and_hour).is_some_and(|(d, h)| d == date && h >= hour))
                .unwrap_or(times.len() - 1);
            let at = |key: &str| {
                hourly.get(key).and_then(Value::as_array).and_then(|v| v.get(idx)).cloned().unwrap_or(Value::Null)
            };
            (
                num(Some(&at("temperature_2m"))),
                num(Some(&at("apparent_temperature"))),
                num(Some(&at("wind_speed_10m"))),
                num(Some(&at("wind_direction_10m"))),
                num(Some(&at("weather_code"))),
            )
        }
        None => {
            let current = data.get("current").ok_or("Open-Meteo current-weather response is malformed.")?;
            (
                num(current.get("temperature_2m")),
                num(current.get("apparent_temperature")),
                num(current.get("wind_speed_10m")),
                num(current.get("wind_direction_10m")),
                num(current.get("weather_code")),
            )
        }
    };
    let (Some(temp), Some(feels), Some(speed), Some(dir)) = (temp, feels, speed, dir) else {
        return Err("Open-Meteo response is missing one or more required fields.".into());
    };
    // The current weather must have a weather code; for a forecast a missing code is allowed
    // and shows as clouds (both as in Python).
    if forecast.is_none() && code.is_none() {
        return Err("Open-Meteo response is missing one or more required fields.".into());
    }
    Ok(Weather {
        temp: standard(temp),
        wind_chill: standard(feels),
        wind_speed: speed,
        wind_speed_symbol: wind_symbol(units).into(),
        wind_dir: dir,
        temp_symbol: temp_symbol(units).into(),
        condition: open_meteo_condition(code).into(),
    })
}

/// One OpenWeather item (the current weather, or one entry of a forecast's list).
/// OpenWeather already converts temperatures to the asked units. Errors name the missing part.
fn openweather_item(item: &Value, units: &str) -> Result<Weather, String> {
    let main = item.get("main").ok_or("OpenWeather response is missing main")?;
    let wind = item.get("wind").ok_or("OpenWeather response is missing wind")?;
    let condition = item
        .get("weather")
        .and_then(|w| w.get(0))
        .and_then(|w| w.get("main"))
        .and_then(Value::as_str)
        .ok_or("OpenWeather response is missing weather[0].main")?;
    let (Some(temp), Some(feels), Some(mut speed), Some(dir)) =
        (num(main.get("temp")), num(main.get("feels_like")), num(wind.get("speed")), num(wind.get("deg")))
    else {
        return Err("OpenWeather response is missing required values".into());
    };
    // m/s to km/h
    if units != "imperial" {
        speed *= 3.6;
    }
    // Other condition names (Clear, Clouds, Rain, ...) pass through; `weather_values` lower-cases
    // them to find the icon.
    let condition = if MIST_LIKE.contains(&condition) { "mist-like".to_string() } else { condition.to_string() };
    Ok(Weather {
        temp,
        wind_chill: feels,
        wind_speed: speed,
        wind_speed_symbol: wind_symbol(units).into(),
        wind_dir: dir,
        temp_symbol: temp_symbol(units).into(),
        condition,
    })
}

/// OpenWeather's answer: `/data/2.5/weather`, or `/data/2.5/forecast` when `forecast` gives the day and hour.
/// For the current weather an answer without "main" is an error answer (for example a bad API key),
/// and its "message" is put in the error. For a forecast (a list of 3-hourly items) the first item on
/// the wanted day at or after the wanted hour is used, or the last item when none matches.
pub fn parse_openweather(data: &Value, units: &str, forecast: Option<(&str, i64)>) -> Result<Weather, String> {
    let message = || data.get("message").map(|m| m.to_string()).unwrap_or_else(|| data.to_string());
    match forecast {
        None => {
            if data.get("main").is_none() {
                return Err(format!("OpenWeather current-weather lookup failed: {}", message()));
            }
            openweather_item(data, units)
        }
        Some((date, hour)) => {
            let list = data.get("list").and_then(Value::as_array).filter(|l| !l.is_empty());
            let list = list.ok_or_else(|| format!("OpenWeather forecast lookup failed: {}", message()))?;
            let item = list
                .iter()
                .find(|fc| {
                    fc.get("dt_txt")
                        .and_then(Value::as_str)
                        .and_then(date_and_hour)
                        .is_some_and(|(d, h)| d == date && h >= hour)
                })
                .unwrap_or(&list[list.len() - 1]);
            openweather_item(item, units)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample reading used by the tests below.
    fn weather() -> Weather {
        Weather {
            temp: 21.4,
            wind_chill: -3.5,
            wind_speed: 12.5,
            wind_speed_symbol: "km".into(),
            wind_dir: 225.0,
            temp_symbol: "degC".into(),
            condition: "Clouds".into(),
        }
    }

    /// Each measure gives the same glyph names as Python, including Python's rounding (-3.5 -> -4,
    /// 12.5 -> 12) and the compass point for 225 degrees (south-west).
    #[test]
    fn values_like_python() {
        let w = weather();
        assert_eq!(weather_values(&w, "temp_condition"), vec!["2", "1", "degC", "clouds"]);
        assert_eq!(weather_values(&w, "wind_chill"), vec!["wc", " ", "-", "4", "degC"]);
        assert_eq!(weather_values(&w, "wind"), vec!["wd", "1", "2", "km", "wind-sw"]);
        assert_eq!(weather_values(&w, "x"), vec!["?", "?"]);
    }

    /// Measures switch every `duration` seconds and wrap around, unknown names are skipped, and with no
    /// known names the temperature shows.
    #[test]
    fn measures_take_turns() {
        let m = ["temp_condition", "bogus", "wind"];
        assert_eq!(measure_at(&m, 10.0, 5.0), "temp_condition");
        assert_eq!(measure_at(&m, 10.0, 15.0), "wind");
        assert_eq!(measure_at(&m, 10.0, 25.0), "temp_condition");
        assert_eq!(measure_at(&["bogus"], 10.0, 15.0), "temp_condition");
    }

    /// The current weather is read with its condition; a forecast picks the right hour, converts to
    /// Kelvin for "standard", and falls back to the last hour for a date past the forecast.
    #[test]
    fn open_meteo_current_and_forecast() {
        let current = serde_json::json!({"current": {"temperature_2m": 10.0, "apparent_temperature": 8.0,
            "wind_speed_10m": 5.0, "wind_direction_10m": 90, "weather_code": 61}});
        let w = parse_open_meteo(&current, "metric", None).unwrap();
        assert_eq!((w.temp, w.condition.as_str(), w.temp_symbol.as_str()), (10.0, "rain", "degC"));
        let forecast = serde_json::json!({"hourly": {"time": ["2026-11-15T09:00", "2026-11-15T12:00", "2026-11-15T15:00"],
            "temperature_2m": [1, 2, 3], "apparent_temperature": [1, 2, 3], "wind_speed_10m": [1, 2, 3],
            "wind_direction_10m": [1, 2, 3], "weather_code": [0, 71, 0]}});
        let w = parse_open_meteo(&forecast, "standard", Some(("2026-11-15", 12))).unwrap();
        assert_eq!((w.temp, w.condition.as_str()), (275.15, "snow"));
        let w = parse_open_meteo(&forecast, "metric", Some(("2026-11-20", 12))).unwrap();
        assert_eq!(w.temp, 3.0);
    }

    /// OpenWeather's m/s become km/h, "Haze" shows as mist-like, and an error answer is an error.
    #[test]
    fn openweather_current() {
        let data = serde_json::json!({"main": {"temp": 5.0, "feels_like": 2.0}, "wind": {"speed": 10.0, "deg": 0},
            "weather": [{"main": "Haze"}]});
        let w = parse_openweather(&data, "metric", None).unwrap();
        assert_eq!((w.wind_speed, w.condition.as_str()), (36.0, "mist-like"));
        assert!(parse_openweather(&serde_json::json!({"message": "bad key"}), "metric", None).is_err());
    }
}
