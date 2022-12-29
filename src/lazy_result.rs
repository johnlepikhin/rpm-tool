use std::{cell::Cell, rc::Rc};

pub struct LazyResult<T, E> {
    value: Cell<Option<Rc<T>>>,
    initializer: Box<dyn Fn() -> Result<T, E>>,
}

impl<T, E> LazyResult<T, E> {
    pub fn new<I>(initializer: I) -> Self
    where
        I: Fn() -> Result<T, E> + 'static,
    {
        Self {
            value: Cell::new(None),
            initializer: Box::new(initializer),
        }
    }

    pub fn get(&self) -> Result<Rc<T>, E> {
        let value = match self.value.take() {
            Some(v) => v,
            None => Rc::new((self.initializer)()?),
        };
        self.value.set(Some(value.clone()));
        Ok(value)
    }
}
