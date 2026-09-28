use crate::inquire_live::ask;
use anyhow::Result;
use forge_app::UserInfra;
use forge_domain::{
    InteractionAnswer, InteractionContext, InteractionKind, PermissionOperation, PolicyPermission,
};
use forge_select::ForgeWidget;

pub struct ForgeInquire;

impl Default for ForgeInquire {
    fn default() -> Self {
        Self::new()
    }
}

impl ForgeInquire {
    pub fn new() -> Self {
        Self
    }

    async fn prompt<T, F>(&self, f: F) -> Result<Option<T>>
    where
        F: FnOnce() -> Result<Option<T>> + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(f).await?
    }
}

#[async_trait::async_trait]
impl UserInfra for ForgeInquire {
    async fn confirm_permission(
        &self,
        message: &str,
        operation: &PermissionOperation,
    ) -> Result<Option<PolicyPermission>> {
        if let Some(context) = InteractionContext::current() {
            let options = [
                PolicyPermission::Accept,
                PolicyPermission::Reject,
                PolicyPermission::AcceptAndRemember,
            ];
            let labels = options.iter().map(ToString::to_string).collect();
            return Ok(
                match ask(
                    context,
                    InteractionKind::Permission { operation: operation.clone() },
                    message,
                    labels,
                )
                .await
                {
                    InteractionAnswer::Choices(indices) => indices
                        .first()
                        .and_then(|index| options.get(*index))
                        .cloned(),
                    _ => None,
                },
            );
        }
        self.select_one_enum::<PolicyPermission>(message).await
    }

    async fn prompt_question(&self, question: &str) -> Result<Option<String>> {
        if let Some(context) = InteractionContext::current() {
            return Ok(
                match ask(context, InteractionKind::Text, question, Vec::new()).await {
                    InteractionAnswer::Text(text) => Some(text),
                    _ => None,
                },
            );
        }
        let question = question.to_string();
        self.prompt(move || ForgeWidget::input(&question).allow_empty(true).prompt())
            .await
    }

    async fn select_one<T: Clone + std::fmt::Display + Send + 'static>(
        &self,
        message: &str,
        options: Vec<T>,
    ) -> Result<Option<T>> {
        if options.is_empty() {
            return Ok(None);
        }

        if let Some(context) = InteractionContext::current() {
            let labels = options.iter().map(ToString::to_string).collect();
            return Ok(
                match ask(context, InteractionKind::SingleChoice, message, labels).await {
                    InteractionAnswer::Choices(indices) => indices
                        .first()
                        .and_then(|index| options.get(*index))
                        .cloned(),
                    _ => None,
                },
            );
        }
        let message = message.to_string();
        self.prompt(move || ForgeWidget::select(&message, options).prompt())
            .await
    }

    async fn select_many<T: std::fmt::Display + Clone + Send + 'static>(
        &self,
        message: &str,
        options: Vec<T>,
    ) -> Result<Option<Vec<T>>> {
        if options.is_empty() {
            return Ok(None);
        }

        if let Some(context) = InteractionContext::current() {
            let labels = options.iter().map(ToString::to_string).collect();
            return Ok(
                match ask(context, InteractionKind::MultipleChoice, message, labels).await {
                    InteractionAnswer::Choices(indices) => Some(
                        indices
                            .into_iter()
                            .map(|index| options[index].clone())
                            .collect(),
                    ),
                    _ => None,
                },
            );
        }
        let message = message.to_string();
        self.prompt(move || ForgeWidget::multi_select(&message, options).prompt())
            .await
    }
}
