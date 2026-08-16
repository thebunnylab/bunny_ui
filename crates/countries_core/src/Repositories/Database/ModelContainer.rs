//
//  ModelContainer.swift
//  CountriesSwiftUI
//

#![allow(non_snake_case)]

use crate::Repositories::Models::{AppSchema, DBModel};
use std::cell::RefCell;
use std::rc::Rc;

/// O storage por modelo (`Mutex<Vec<T>>` virou `RefCell<Vec<T>>` —
/// single-thread como o `@MainActor` do app). `countries` fica num `Rc`
/// próprio porque é o que o `Query<DBModel.Country>` do runtime lê via
/// `ProvidesQueries`.
#[derive(Default)]
pub struct DBStorage {
    pub countries: Rc<RefCell<Vec<DBModel::Country>>>,
    pub details: Vec<DBModel::CountryDetails>,
    pub currencies: Vec<DBModel::Currency>,
}

/// `ModelContainer` fake (SwiftData) — in-memory, um storage compartilhado.
#[derive(Clone)]
pub struct ModelContainer {
    /// `configurations.first?.name` (só para o `isStub`)
    pub name: Option<String>,
    pub schema: AppSchema::Schema,
    storage: Rc<RefCell<DBStorage>>,
}

impl ModelContainer {
    /// `static func appModelContainer(inMemoryOnly:isStub:) throws`
    pub fn appModelContainer(_inMemoryOnly: bool, isStub: bool) -> Result<ModelContainer, String> {
        let schema = AppSchema::Schema::appSchema();
        let name = if isStub { Some("stub".to_string()) } else { None };
        Ok(ModelContainer {
            name,
            schema,
            storage: Rc::new(RefCell::new(DBStorage::default())),
        })
    }

    /// `static var stub: ModelContainer`
    pub fn stub() -> ModelContainer {
        Self::appModelContainer(true, true).expect("in-memory container always builds")
    }

    /// `var isStub: Bool { configurations.first?.name == "stub" }`
    pub fn isStub(&self) -> bool {
        self.name.as_deref() == Some("stub")
    }

    pub fn storage(&self) -> Rc<RefCell<DBStorage>> {
        self.storage.clone()
    }
}

/// `FetchDescriptor(predicate: #Predicate { … })` — closure no lugar do macro.
pub struct FetchDescriptor<T> {
    pub predicate: Box<dyn Fn(&T) -> bool>,
}

impl<T> FetchDescriptor<T> {
    pub fn new(predicate: impl Fn(&T) -> bool + 'static) -> Self {
        FetchDescriptor { predicate: Box::new(predicate) }
    }
}

/// `ModelContext` fake — fetch/insert/transaction direto no storage.
#[derive(Clone)]
pub struct ModelContext {
    storage: Rc<RefCell<DBStorage>>,
}

impl ModelContext {
    pub fn new(container: &ModelContainer) -> Self {
        ModelContext { storage: container.storage() }
    }

    /// `modelContext.fetch(fetchDescriptor)`
    pub fn fetchCountries(&self, descriptor: &FetchDescriptor<DBModel::Country>) -> Vec<DBModel::Country> {
        let storage = self.storage.borrow();
        storage
            .countries
            .borrow()
            .iter()
            .filter(|c| (descriptor.predicate)(c))
            .cloned()
            .collect()
    }

    pub fn fetchDetails(
        &self,
        descriptor: &FetchDescriptor<DBModel::CountryDetails>,
    ) -> Vec<DBModel::CountryDetails> {
        self.storage
            .borrow()
            .details
            .iter()
            .filter(|d| (descriptor.predicate)(d))
            .cloned()
            .collect()
    }

    /// `modelContext.insert(model)`
    pub fn insertCountry(&self, country: DBModel::Country) {
        self.storage.borrow().countries.borrow_mut().push(country);
    }

    pub fn insertDetails(&self, details: DBModel::CountryDetails) {
        self.storage.borrow_mut().details.push(details);
    }

    pub fn insertCurrency(&self, currency: DBModel::Currency) {
        self.storage.borrow_mut().currencies.push(currency);
    }

    /// `modelContext.transaction { … }`
    pub fn transaction(&self, body: impl FnOnce(&ModelContext)) {
        body(self);
    }
}

/// `@ModelActor final actor MainDBRepository { }` — o "ator" virou um struct
/// com o container (tudo roda na thread única do app).
pub struct MainDBRepository {
    pub modelContainer: ModelContainer,
    pub modelContext: ModelContext,
}

impl MainDBRepository {
    /// `MainDBRepository(modelContainer:)`
    pub fn new(modelContainer: ModelContainer) -> Self {
        let modelContext = ModelContext::new(&modelContainer);
        MainDBRepository { modelContainer, modelContext }
    }
}

/// `.modelContainer(container)` — expõe o storage de cada modelo para o
/// `Query<T>` do runtime (chaveado por `type_name::<T>()`).
impl motor::state::ProvidesQueries for ModelContainer {
    fn querySource(&self) -> Rc<dyn Fn(&'static str) -> Option<Rc<dyn std::any::Any>>> {
        let countries = self.storage().borrow().countries.clone();
        Rc::new(move |type_name| {
            if type_name == std::any::type_name::<DBModel::Country>() {
                Some(countries.clone())
            } else {
                None
            }
        })
    }
}
