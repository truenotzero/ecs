use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

pub type Id = usize;

// Entities are created by EntityManager::spawn
// To add Components to an Entity, call Component::add
// Entities exist solely to track their lifetime
// Once an entity is dropped, each of its components is cleaned up
//
// To access an Entity's Id to implement ComponentManagers, dereference it
pub struct Entity<'a> {
    id: Id,
    cleanup: Box<dyn Fn(Id) + 'a>,
}

impl<'a> Entity<'a> {
    // internal, for use with EntityManager::spawn
    fn new(id: Id, manager: &'a EntityManager) -> Self {
        let cleanup = Box::new(|n| { manager.destroy(n) });
        Self { id, cleanup }
    }
}

impl<'a> Deref for Entity<'a> {
    type Target=Id;

    fn deref(&self) -> &Self::Target {
        &self.id
    }
}

// Since Drop::drop takes &mut self the cleanup function cannot be FnOnce
// (which would require a `self` receiver, as opposed to drop's `&mut self`)
impl<'a> Drop for Entity<'a> {
    fn drop(&mut self) {
        (self.cleanup)(self.id);
    }
}

// Holds the 'brains' of the entity manager, but, due to the type system's constraints
// to have the ECS work (specifically: entity cleanup needing to hold a reference to EntityManager),
// requires interior mutability, but this constraint should be transparent to the user
// For API use, see EntityManager
#[derive(Default)]
struct EntityManagerCore<'a> {
    counter: Id,
    component_cleanups: Vec<Box<dyn FnMut(Id) + 'a>>,
}


// EntityManager::default() to create
// EntityManager must outlive components registered with register_component
// It is recommended to use EntityManager as a global
#[derive(Default, Clone)]
pub struct EntityManager<'a>(Rc<RefCell<EntityManagerCore<'a>>>);

impl<'a> EntityManager<'a> {
    pub fn spawn(&mut self) -> Entity {
        let mut this = self.0.borrow_mut();
        let ret = Entity::new(this.counter, self);
        this.counter += 1;
        ret
    }

    pub fn register_component<C: Component, M: ComponentManager<C>>(&mut self, m: &'a mut M) {
        let mut this = self.0.try_borrow_mut().unwrap();
        this.component_cleanups.push(Box::new(|e| m.remove(e)));
    }

    fn destroy(&self, e: Id) {
        let mut this = self.0.borrow_mut();
        for c in this.component_cleanups.iter_mut() {
            c(e);
        }
    }
}

// Components store data for entities
// In memory, components are modeled as SoA (struct of arrays)
// Consequently, to have a nicer API, for each Component struct,
// we need a struct of references (Ref) and a struct of mutable references (RefMut)
// This trait aims to provide a single interface linking all three types
// It is recommended to use the component! macro to implement components
//TODO: find a way to enforce that RefMut must also implement From<(anonymous tuple of the component's fields)>
pub trait Component: Default {
    type Ref<'a>;
    type RefMut<'a>;
}

// ComponentManager is the data store for components, the interface to the SoA
// effectively, the trait creates a connection between the Component and its storage
// The recommended use case is:
// for each field (where field: F) in the Component there should be a matching HashMap<Id, F>
// Note, however, that it is up to the user to implement the data however they wish
// as long as the function's contracts are upheld
pub trait ComponentManager<C: Component> {
    fn add(&mut self, e: Entity, c: Option<C>);
    fn remove(&mut self, e: Id);

    fn iter(&self) -> impl Iterator<Item=C::Ref<'_>>;
    fn iter_mut(&mut self) -> impl Iterator<Item=C::RefMut<'_>>;
}

// There is no System trait as there is no need for one!
// However, for easy use, a system! macro is provided
// A system can be implemented like so:
// struct RenderSystem<'a> {
//     transforms: &'a TransformManager,
//     meshes: &'a MeshManager,
// }
//
// impl<'a> RenderSystem<'a> {
//     fn iter(&self) -> impl Iterator<Item=(<Transform as Component>::Ref<'a>, <Mesh as Component>::Ref<'a>)> {
//         izip!(self.transforms.iter(), self.meshes.iter())
//     }
// }
//
// impl<'a> RenderSystem<'a> {
//     fn draw(&self) {
//         for (t, m) in self.iter() {
//             // use t,m in here
//         }
//     }
// }

// Define your component struct inside the macro to automatically
// implement the Component trait (and required Ref glue) for it
// Caveats:
// The struct must be default-derivable
// Each field must be incrementally annotated with #[slot(n)] starting at 0
// The last field must have a comma after it (struct Foo { x: i32, })
// Does not support struct visibility modifiers (pub struct Foo)
// Does not support field visibility modifiers (struct Foo { pub x: () })
// Does not work with generics/lifetimes (struct<T> Foo { ... })
// Doess not work with tuple structs (struct Foo(...))
// Does not catch attributes (#[derive(...)] for example)
#[macro_export]
macro_rules! component {
    (struct $name:ident { $( #[slot($s:literal)] $field_name:ident: $field_type:ty, )+ }) => {
        // implement struct
        #[derive(Default)]
        pub struct $name {
        $( pub $field_name: $field_type ),+
        }

        $crate::paste::paste! {
        // create Ref/RefMut glue
        pub struct [< $name Ref >]<'a> {
        $( pub $field_name: &'a $field_type ),+
        }

        pub struct [< $name RefMut >]<'a> {
        $( pub $field_name: &'a mut $field_type ),+
        }

        // implement From(tuple) for glue types (iterator support)
        impl<'a> From<( $(&'a $field_type),+ )> for [< $name Ref >]<'a> {
            fn from(value: ( $(&'a $field_type),+ )) -> Self {
                Self {
                    $(
                    $field_name: value.$s,
                    )+
                }
            }
        }

        impl<'a> From<( $(&'a mut $field_type),+ )> for [< $name RefMut >]<'a> {
            fn from(value: ( $(&'a mut $field_type),+ )) -> Self {
                Self {
                    $(
                    $field_name: value.$s,
                    )+
                }
            }
        }

        // implement Component
        impl Component for $name {
            type Ref<'a> = [< $name Ref >]<'a>;
            type RefMut<'a> = [< $name RefMut >]<'a>;
        }

        // implement SoA
        #[derive(Default)]
        pub struct [< $name Manager >] {
        $( $field_name: std::collections::HashMap<$crate::ecs::Id, $field_type> ),+
        }

        // implement ComponentManager for SoA
        impl ComponentManager<$name> for [< $name Manager >] {
            fn add(&mut self, e: $crate::ecs::Entity, default: Option<$name>) {
                let default = default.unwrap_or_default();
                $(
                self.$field_name.insert(*e, default.$field_name);
                )+
            }

            fn remove(&mut self, e: $crate::ecs::Id) {
                $(
                self.$field_name.remove(&e);
                )+
            }

            fn iter(&self) -> impl Iterator<Item=<$name as Component>::Ref<'_>>{
                $crate::itertools::izip! {
                $( self.$field_name.values() ),+
                }
                .map(From::from)
            }

            fn iter_mut(&mut self) -> impl Iterator<Item=<$name as Component>::RefMut<'_>>{
                $crate::itertools::izip! {
                $(  self.$field_name.values_mut() ),+
                }
                .map(From::from)
            }
        }
        }
    };
}
