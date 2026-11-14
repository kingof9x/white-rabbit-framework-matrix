//! The weather app (`led_mon/plugins/time_weather_plugin.py`): what it shows for a reading, and
//! reading the answers of the two weather services it asks (Open-Meteo, and OpenWeather when an
//! API key is set). Asking them is in `wr-monitor`.

use crate::pyround::int_round;
use crate::Grid;
use serde_json::Value;

/// One weather reading, in the units picked for the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub temp: f64,
    pub wind_chill: f64,
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

pub const MEASURES: [&str; 3] = ["temp_condition", "wind_chill", "wind"];
const MIST_LIKE: [&str; 9] = ["Mist", "Fog", "Dust", "Haze", "Smoke", "Squall", "Ash", "Sand", "Tornado"];

/// Python's str(round(x)) as single characters: "-", "4".
fn digits(x: f64) -> Vec<String> {
    int_round(x).to_string().chars().map(|c| c.to_string()).collect()
}

/// The characters the app draws for one measure, one under the other.
pub fn weather_values(w: &Weather, measure: &str) -> Vec<String> {
    match measure {
        "temp_condition" => {
            let mut v = digits(w.temp);
            v.push(w.temp_symbol.clone());
            v.push(w.condition.to_lowercase());
            v
        }
        "wind_chill" => {
            let mut v = vec!["wc".to_string(), " ".to_string()];
            v.extend(digits(w.wind_chill));
            v.push(w.temp_symbol.clone());
            v
        }
        "wind" => {
            const DIRS: [&str; 8] = ["n", "ne", "e", "se", "s", "sw", "w", "nw"];
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
pub fn measure_at<'a>(measures: &[&'a str], duration: f64, now: f64) -> &'a str {
    let valid: Vec<&str> = measures.iter().copied().filter(|m| MEASURES.contains(m)).collect();
    if valid.is_empty() {
        return "temp_condition";
    }
    if duration <= 0.0 {
        return valid[0];
    }
    valid[((now / duration).floor().max(0.0) as usize) % valid.len()]
}

/// Bars up the bottom of the left and right edges for the forecast's day (1-5, left) and hour
/// (right, one row per 3 hours), with a mark beside the fourth row of the hour bar.
pub fn draw_forecast_period(grid: &mut Grid, fill_value: i32, day: i64, hour: i64) {
    let day = day.clamp(1, 5);
    let mut hours_idx = 8;
    for (idx, h) in [0, 3, 6, 9, 12, 15, 18, 21].iter().enumerate() {
        if hour >= *h {
            hours_idx = idx;
        }
    }
    let hours_idx = hours_idx as i64 + 1;
    grid.fill_y(0, 33 - day + 1, 34, fill_value);
    grid.fill_y(8, 33 - hours_idx + 1, 34, fill_value);
    grid.set(7, 30, grid.get(8, 30));
}

pub fn temp_symbol(units: &str) -> &'static str {
    match units {
        "metric" => "degC",
        "imperial" => "degF",
        _ => "degK",
    }
}

fn wind_symbol(units: &str) -> &'static str {
    if units == "imperial" {
        "mi"
    } else {
        "km"
    }
}

/// Open-Meteo's WMO weather code as an icon name.
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

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

/// "YYYY-MM-DD" and the hour of an ISO time such as "2026-11-14T15:00" or "2026-11-14 15:00:00".
fn date_and_hour(text: &str) -> Option<(&str, i64)> {
    let date = text.get(..10)?;
    let hour = text.get(11..13)?.parse().ok()?;
    Some((date, hour))
}

/// Open-Meteo's answer. `forecast_date` (YYYY-MM-DD, GMT) and `forecast_hour` pick the forecast hour:
/// the first one on that day at or after that hour, otherwise the last one given.
pub fn parse_open_meteo(data: &Value, units: &str, forecast: Option<(&str, i64)>) -> Result<Weather, String> {
    if data.get("error").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("Open-Meteo lookup failed: {}", data.get("reason").unwrap_or(data)));
    }
    let standard = |v: f64| if units == "standard" { v + 273.15 } else { v };
    let (temp, feels, speed, dir, code) = match forecast {
        Some((date, hour)) => {
            let hourly = data.get("hourly").ok_or("Open-Meteo forecast has no hourly data")?;
            let times = hourly.get("time").and_then(Value::as_array).filter(|t| !t.is_empty());
            let times = times.ok_or("Open-Meteo forecast response did not include hourly time data.")?;
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

    #[test]
    fn values_like_python() {
        let w = weather();
        assert_eq!(weather_values(&w, "temp_condition"), vec!["2", "1", "degC", "clouds"]);
        assert_eq!(weather_values(&w, "wind_chill"), vec!["wc", " ", "-", "4", "degC"]);
        assert_eq!(weather_values(&w, "wind"), vec!["wd", "1", "2", "km", "wind-sw"]);
        assert_eq!(weather_values(&w, "x"), vec!["?", "?"]);
    }

    #[test]
    fn measures_take_turns() {
        let m = ["temp_condition", "bogus", "wind"];
        assert_eq!(measure_at(&m, 10.0, 5.0), "temp_condition");
        assert_eq!(measure_at(&m, 10.0, 15.0), "wind");
        assert_eq!(measure_at(&m, 10.0, 25.0), "temp_condition");
        assert_eq!(measure_at(&["bogus"], 10.0, 15.0), "temp_condition");
    }

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

    #[test]
    fn openweather_current() {
        let data = serde_json::json!({"main": {"temp": 5.0, "feels_like": 2.0}, "wind": {"speed": 10.0, "deg": 0},
            "weather": [{"main": "Haze"}]});
        let w = parse_openweather(&data, "metric", None).unwrap();
        assert_eq!((w.wind_speed, w.condition.as_str()), (36.0, "mist-like"));
        assert!(parse_openweather(&serde_json::json!({"message": "bad key"}), "metric", None).is_err());
    }
}
