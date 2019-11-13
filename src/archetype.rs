use std::alloc::{alloc, Layout};
use std::any::TypeId;
use std::cell::UnsafeCell;
use std::ptr::{self, NonNull};

use fxhash::FxHashMap;

use crate::{Component, ComponentSet};

/// A collection of entities having the same component types
pub struct Archetype {
    types: Vec<TypeInfo>,
    offsets: FxHashMap<TypeId, usize>,
    len: u32,
    entities: Box<[u32]>,
    // UnsafeCell allows unique references into `data` to be constructed while shared references
    // containing the `Archetype` exist
    data: UnsafeCell<Box<[u8]>>,
}

impl Archetype {
    pub fn new(types: Vec<TypeInfo>) -> Self {
        Self {
            types,
            offsets: FxHashMap::default(),
            entities: Box::new([]),
            len: 0,
            data: UnsafeCell::new(Box::new([])),
        }
    }

    pub fn data<T: Component>(&self) -> Option<NonNull<T>> {
        let offset = *self.offsets.get(&TypeId::of::<T>())?;
        Some(unsafe {
            NonNull::new_unchecked((*self.data.get()).as_ptr().add(offset).cast::<T>() as *mut T)
        })
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn entities(&self) -> NonNull<u32> {
        unsafe { NonNull::new_unchecked(self.entities.as_ptr() as *mut _) }
    }

    pub fn types(&self) -> &[TypeInfo] {
        &self.types
    }

    /// `index` must be in-bounds and live
    pub unsafe fn get<T: Component>(&self, index: u32) -> &T {
        debug_assert!(index < self.len);
        &*self
            .data::<T>()
            .expect("no such component")
            .as_ptr()
            .add(index as usize)
    }

    /// `index` must be in-bounds and live and not aliased
    pub unsafe fn get_mut<T: Component>(&self, index: u32) -> &mut T {
        debug_assert!(index < self.len);
        &mut *self
            .data::<T>()
            .expect("no such component")
            .as_ptr()
            .add(index as usize)
    }

    /// Every type must be written immediately after this call
    pub unsafe fn allocate(&mut self, id: u32) -> u32 {
        if (self.len as usize) < self.entities.len() {
            self.entities[self.len as usize] = id;
            self.len += 1;
            return self.len - 1;
        }

        // At this point we need to allocate more storage.
        let count = if self.entities.len() == 0 {
            64
        } else {
            self.entities.len() * 2
        };
        let mut new_entities = vec![!0; count].into_boxed_slice();
        new_entities[0..self.entities.len()].copy_from_slice(&self.entities);
        self.entities = new_entities;

        let mut data_size = 0;
        let mut offsets = FxHashMap::default();
        for ty in &self.types {
            data_size = align(data_size, ty.layout.align());
            offsets.insert(ty.id, data_size);
            data_size += ty.layout.size() * count;
        }
        let alloc = alloc(
            Layout::from_size_align(
                data_size,
                self.types.first().map_or(1, |x| x.layout.align()),
            )
            .unwrap(),
        );
        let mut new_data = Box::from_raw(std::slice::from_raw_parts_mut(alloc, data_size));
        if !(*self.data.get()).is_empty() {
            for ty in &self.types {
                let old_off = *self.offsets.get(&ty.id).unwrap();
                let new_off = *offsets.get(&ty.id).unwrap();
                ptr::copy_nonoverlapping(
                    (*self.data.get()).as_ptr().add(old_off),
                    new_data.as_mut_ptr().add(new_off),
                    ty.layout.size() * self.entities.len(),
                );
            }
        }

        self.data = UnsafeCell::new(new_data);
        self.offsets = offsets;
        self.entities[self.len as usize] = id;
        self.len += 1;
        self.len - 1
    }

    /// Returns the ID of the entity moved into `index`, if any
    pub unsafe fn remove(&mut self, index: u32) -> Option<u32> {
        let last = self.len - 1;
        for ty in &self.types {
            let base = (*self.data.get())
                .as_mut_ptr()
                .add(*self.offsets.get(&ty.id).unwrap());
            let removed = base.add(ty.layout.size() * index as usize);
            (ty.drop)(removed);
            if index != last {
                ptr::copy_nonoverlapping(
                    base.add(ty.layout.size() * last as usize),
                    removed,
                    ty.layout.size(),
                );
            }
        }
        self.len = last;
        if index != last {
            self.entities[index as usize] = self.entities[last as usize];
            Some(self.entities[last as usize])
        } else {
            None
        }
    }

    /// Extract a component prior to the entity being moved to an archetype that lacks it
    pub unsafe fn read<T: Component>(&mut self, index: u32) -> T {
        self.data::<T>()
            .expect("no such component")
            .as_ptr()
            .add(index as usize)
            .read()
    }

    pub unsafe fn move_component_set(&mut self, index: u32) -> EntityComponentSet {
        EntityComponentSet {
            archetype: self,
            index,
        }
    }

    unsafe fn move_to(&mut self, index: u32, target: &mut Archetype, target_index: u32) {
        let last = self.len - 1;
        for ty in &self.types {
            let base = (*self.data.get())
                .as_mut_ptr()
                .add(*self.offsets.get(&ty.id).unwrap());
            let moved = base.add(ty.layout.size() * index as usize);
            // Tolerate missing components
            if target.offsets.contains_key(&ty.id) {
                target.put_dynamic(moved, ty.id, ty.layout, target_index);
            }
            if index != last {
                ptr::copy_nonoverlapping(
                    base.add(ty.layout.size() * last as usize),
                    moved,
                    ty.layout.size(),
                );
            }
        }
        if index != last {
            self.entities[index as usize] = self.entities[last as usize];
        }
        self.len -= 1;
    }

    pub unsafe fn put<T: Component>(&mut self, component: T, index: u32) {
        self.data::<T>()
            .unwrap()
            .as_ptr()
            .add(index as usize)
            .write(component);
    }

    pub unsafe fn put_dynamic(
        &mut self,
        component: *mut u8,
        ty: TypeId,
        layout: Layout,
        index: u32,
    ) {
        let offset = *self.offsets.get(&ty).unwrap();
        let ptr = (*self.data.get())
            .as_mut_ptr()
            .add(offset + layout.size() * index as usize);
        ptr::copy_nonoverlapping(component, ptr, layout.size());
    }
}

impl Drop for Archetype {
    fn drop(&mut self) {
        for i in 0..self.len {
            if self.entities[i as usize] != !0 {
                unsafe {
                    self.remove(i);
                }
            }
        }
    }
}

fn align(x: usize, alignment: usize) -> usize {
    assert!(alignment.is_power_of_two());
    (x + alignment - 1) & (!alignment + 1)
}

#[derive(Debug, Clone)]
pub struct TypeInfo {
    id: TypeId,
    layout: Layout,
    drop: unsafe fn(*mut u8),
}

impl TypeInfo {
    pub fn id(&self) -> TypeId {
        self.id
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }
}

impl PartialOrd for TypeInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TypeInfo {
    /// Order by alignment, descending. Ties broken with TypeId.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.layout
            .align()
            .cmp(&other.layout.align())
            .reverse()
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialEq for TypeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeInfo {}

impl TypeInfo {
    pub fn of<T: 'static>() -> Self {
        unsafe fn drop_ptr<T>(x: *mut u8) {
            x.cast::<T>().drop_in_place()
        }

        Self {
            id: TypeId::of::<T>(),
            layout: Layout::new::<T>(),
            drop: drop_ptr::<T>,
        }
    }
}

pub struct EntityComponentSet<'a> {
    archetype: &'a mut Archetype,
    index: u32,
}

impl<'a> ComponentSet for EntityComponentSet<'a> {
    fn elements(&self) -> Vec<TypeId> {
        self.archetype.types.iter().map(|x| x.id).collect()
    }
    fn info(&self) -> Vec<TypeInfo> {
        self.archetype.types.clone()
    }
    unsafe fn store(self, archetype: &mut Archetype, index: u32) {
        self.archetype.move_to(self.index, archetype, index);
    }
}