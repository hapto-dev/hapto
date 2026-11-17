# // A parameter made at run time: a plugin makes one with `Parts::declare` or
# // `Parts::add`, in `hapto-host`, which this crate cannot reach.
# let (volume, _) = hapto_params::Declarer::run_time_one(
#     &set,
#     Box::new(|_| None),
#     hapto_params::FloatDecl::new("volume", "Volume", -60.0..=12.0, 0.0),
# );
