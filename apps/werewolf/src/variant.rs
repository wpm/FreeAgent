mod random;

trait Step {
    fn night(&self) -> anyhow::Result<()>;
    fn day(&self) -> anyhow::Result<()>;
}