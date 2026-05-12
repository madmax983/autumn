use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Posts::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Posts::Id).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(Posts::Title).text().not_null())
                    .col(ColumnDef::new(Posts::Body).text().not_null())
                    .col(ColumnDef::new(Posts::Published).boolean().not_null().default(false))
                    .col(ColumnDef::new(Posts::Author).text().not_null())
                    .col(ColumnDef::new(Posts::CreatedAt).timestamp_with_time_zone().not_null().extra("DEFAULT NOW()".to_string()))
                    .col(ColumnDef::new(Posts::UpdatedAt).timestamp_with_time_zone().not_null().extra("DEFAULT NOW()".to_string()))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiTokens::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ApiTokens::Id).big_integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(ApiTokens::Token).text().not_null().unique_key())
                    .col(ColumnDef::new(ApiTokens::Principal).text().not_null())
                    .col(ColumnDef::new(ApiTokens::CreatedAt).timestamp_with_time_zone().not_null().extra("DEFAULT NOW()".to_string()))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Posts::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(ApiTokens::Table).to_owned()).await
    }
}

#[derive(DeriveIden)]
enum Posts { Table, Id, Title, Body, Published, Author, CreatedAt, UpdatedAt }

#[derive(DeriveIden)]
enum ApiTokens { Table, Id, Token, Principal, CreatedAt }
