//! Audio analysis: versioned features, the worker protocol and the process
//! runner that keeps analysis failures away from the app.

pub mod handler;
pub mod protocol;
pub mod runner;
pub mod store;

use serde::{Deserialize, Serialize};

/// Everything that makes two embeddings comparable. Vectors from different
/// versions are never compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeatureVersion {
    pub model_id: String,
    pub weights_checksum: String,
    pub preprocessing_version: String,
}

impl std::fmt::Display for FeatureVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, {})",
            self.model_id, self.preprocessing_version, self.weights_checksum
        )
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("cannot compare embeddings from {a} and {b}")]
pub struct IncompatibleVersions {
    pub a: FeatureVersion,
    pub b: FeatureVersion,
}

/// An embedding tied to the version that produced it. There is no way to
/// compare the raw vectors of two embeddings except through
/// [`Embedding::similarity`], which checks the versions first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    version: FeatureVersion,
    vector: Vec<f32>,
}

impl Embedding {
    pub fn new(version: FeatureVersion, vector: Vec<f32>) -> Self {
        Embedding { version, vector }
    }

    pub fn version(&self) -> &FeatureVersion {
        &self.version
    }

    pub fn dims(&self) -> usize {
        self.vector.len()
    }

    /// Little-endian f32 bytes for storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.vector.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    pub fn from_bytes(version: FeatureVersion, bytes: &[u8]) -> Self {
        Embedding {
            version,
            vector: bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        }
    }

    /// Cosine similarity, only between embeddings of the same version and
    /// dimension.
    pub fn similarity(&self, other: &Embedding) -> Result<f32, Box<IncompatibleVersions>> {
        if self.version != other.version || self.vector.len() != other.vector.len() {
            return Err(Box::new(IncompatibleVersions {
                a: self.version.clone(),
                b: other.version.clone(),
            }));
        }
        let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
        for (x, y) in self.vector.iter().zip(&other.vector) {
            dot += (*x as f64) * (*y as f64);
            na += (*x as f64).powi(2);
            nb += (*y as f64).powi(2);
        }
        if na == 0.0 || nb == 0.0 {
            return Ok(0.0);
        }
        Ok((dot / (na.sqrt() * nb.sqrt())) as f32)
    }

    /// A unit-length copy, so repeated comparisons need only a dot product.
    pub fn unit(&self) -> UnitEmbedding {
        let norm = self
            .vector
            .iter()
            .map(|x| (*x as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        let scale = if norm > 0.0 { (1.0 / norm) as f32 } else { 0.0 };
        UnitEmbedding(Embedding {
            version: self.version.clone(),
            vector: self.vector.iter().map(|x| x * scale).collect(),
        })
    }
}

/// A unit-length embedding. Comparison still checks versions.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitEmbedding(Embedding);

impl UnitEmbedding {
    pub fn version(&self) -> &FeatureVersion {
        &self.0.version
    }

    pub fn values(&self) -> &[f32] {
        &self.0.vector
    }

    /// Cosine similarity with another unit embedding of the same version.
    pub fn cosine(&self, other: &UnitEmbedding) -> Result<f32, Box<IncompatibleVersions>> {
        if self.0.version != other.0.version || self.0.vector.len() != other.0.vector.len() {
            return Err(Box::new(IncompatibleVersions {
                a: self.0.version.clone(),
                b: other.0.version.clone(),
            }));
        }
        Ok(self
            .0
            .vector
            .iter()
            .zip(&other.0.vector)
            .map(|(a, b)| a * b)
            .sum())
    }

    /// The unit mean of several unit embeddings of this version.
    pub fn centroid(members: &[&UnitEmbedding]) -> Option<UnitEmbedding> {
        let first = members.first()?;
        let dims = first.0.vector.len();
        let mut sum = vec![0f32; dims];
        for m in members
            .iter()
            .filter(|m| m.0.version == first.0.version && m.0.vector.len() == dims)
        {
            for (s, v) in sum.iter_mut().zip(&m.0.vector) {
                *s += v;
            }
        }
        Some(Embedding::new(first.0.version.clone(), sum).unit())
    }
}

#[cfg(test)]
mod store_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn v(model: &str) -> FeatureVersion {
        FeatureVersion {
            model_id: model.into(),
            weights_checksum: "sha256:0".into(),
            preprocessing_version: "p1".into(),
        }
    }

    #[test]
    fn same_version_embeddings_compare() {
        let a = Embedding::new(v("m"), vec![1.0, 0.0]);
        let b = Embedding::new(v("m"), vec![1.0, 1.0]);
        assert!((a.similarity(&b).unwrap() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert_eq!(a.similarity(&a).unwrap(), 1.0);
    }

    #[test]
    fn incompatible_versions_are_refused() {
        let a = Embedding::new(v("m1"), vec![1.0, 0.0]);
        let b = Embedding::new(v("m2"), vec![1.0, 0.0]);
        assert!(a.similarity(&b).is_err());
        let mut other = v("m1");
        other.preprocessing_version = "p2".into();
        assert!(a.similarity(&Embedding::new(other, vec![1.0, 0.0])).is_err());
        let mut weights = v("m1");
        weights.weights_checksum = "sha256:1".into();
        assert!(a.similarity(&Embedding::new(weights, vec![1.0, 0.0])).is_err());
        // Same version but a different dimension is also refused.
        assert!(a
            .similarity(&Embedding::new(v("m1"), vec![1.0, 0.0, 0.0]))
            .is_err());
    }

    #[test]
    fn bytes_round_trip() {
        let a = Embedding::new(v("m"), vec![0.25, -1.5, 3.0]);
        assert_eq!(Embedding::from_bytes(v("m"), &a.to_bytes()), a);
    }
}
