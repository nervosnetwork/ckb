use crate::versionbits::{Deployment, DeploymentPos, ThresholdState};
use ckb_jsonrpc_types::{
    self, DeploymentInfo, DeploymentPos as JsonDeploymentPos, DeploymentState,
};

impl From<ThresholdState> for DeploymentState {
    fn from(state: ThresholdState) -> Self {
        match state {
            ThresholdState::Defined => DeploymentState::Defined,
            ThresholdState::Started => DeploymentState::Started,
            ThresholdState::LockedIn => DeploymentState::LockedIn,
            ThresholdState::Active => DeploymentState::Active,
            ThresholdState::Failed => DeploymentState::Failed,
        }
    }
}

impl From<Deployment> for DeploymentInfo {
    fn from(deployment: Deployment) -> Self {
        DeploymentInfo {
            bit: deployment.bit,
            start: deployment.start.into(),
            timeout: deployment.timeout.into(),
            min_activation_epoch: deployment.min_activation_epoch.into(),
            period: deployment.period.into(),
            threshold: deployment.threshold,
            state: DeploymentState::Defined,
            since: 0.into(),
        }
    }
}

impl From<Deployment> for ckb_jsonrpc_types::Deployment {
    fn from(deployment: Deployment) -> Self {
        ckb_jsonrpc_types::Deployment {
            bit: deployment.bit,
            start: deployment.start.into(),
            timeout: deployment.timeout.into(),
            min_activation_epoch: deployment.min_activation_epoch.into(),
            period: deployment.period.into(),
            threshold: deployment.threshold,
        }
    }
}

impl From<DeploymentPos> for JsonDeploymentPos {
    fn from(pos: DeploymentPos) -> Self {
        match pos {
            DeploymentPos::Testdummy => JsonDeploymentPos::Testdummy,
            DeploymentPos::LightClient => JsonDeploymentPos::LightClient,
        }
    }
}
