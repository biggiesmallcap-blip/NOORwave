use crate::RhythmError;
use ndarray::{Array1, Array2, ArrayD, ArrayView1, ArrayView2, Ix1, Ix2, IxDyn, OwnedRepr};
use ndarray_npy::NpzReader;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek};

pub struct NpzArrays {
    arrays: HashMap<String, ArrayD<f32>>,
}

impl NpzArrays {
    pub fn open(path: &str) -> Result<Self, RhythmError> {
        let file = File::open(path).map_err(|e| RhythmError::Io(e.to_string()))?;
        Self::from_reader(file)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RhythmError> {
        Self::from_reader(Cursor::new(bytes))
    }

    pub fn array1_view(&self, name: &str) -> Result<ArrayView1<'_, f32>, RhythmError> {
        self.by_name(name)?
            .view()
            .into_dimensionality::<Ix1>()
            .map_err(|e| RhythmError::Io(e.to_string()))
    }

    pub fn array2_view(&self, name: &str) -> Result<ArrayView2<'_, f32>, RhythmError> {
        self.by_name(name)?
            .view()
            .into_dimensionality::<Ix2>()
            .map_err(|e| RhythmError::Io(e.to_string()))
    }

    pub fn array1(&self, name: &str) -> Result<Array1<f32>, RhythmError> {
        Ok(self.array1_view(name)?.to_owned())
    }

    pub fn array2(&self, name: &str) -> Result<Array2<f32>, RhythmError> {
        Ok(self.array2_view(name)?.to_owned())
    }

    fn from_reader<R: Read + Seek>(reader: R) -> Result<Self, RhythmError> {
        let mut npz = NpzReader::new(reader).map_err(|e| RhythmError::Io(e.to_string()))?;
        let names = npz.names().map_err(|e| RhythmError::Io(e.to_string()))?;
        let mut arrays = HashMap::with_capacity(names.len());
        for name in names {
            let array = npz
                .by_name::<OwnedRepr<f32>, IxDyn>(&name)
                .map_err(|e| RhythmError::Io(e.to_string()))?;
            arrays.insert(normalize_key(&name), array);
        }
        Ok(Self { arrays })
    }

    fn by_name(&self, name: &str) -> Result<&ArrayD<f32>, RhythmError> {
        let key = normalize_key(name);
        self.arrays
            .get(&key)
            .ok_or_else(|| RhythmError::Io(format!("array not found in npz: {}", name)))
    }
}

fn normalize_key(name: &str) -> String {
    name.strip_suffix(".npy").unwrap_or(name).to_string()
}
