//! Print the default [`VehicleConfig`] as TOML.
//!
//! Used to regenerate `examples/vehicles/dev-car.toml`:
//! `cargo run -p mm2_vehicle --example dump_default_config > examples/vehicles/dev-car.toml`

fn main() {
    print!("{}", mm2_vehicle::VehicleConfig::default().to_toml());
}
