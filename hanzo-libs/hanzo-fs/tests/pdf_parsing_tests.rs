use hanzo_vector_resources::{
    embedding_generator::{EmbeddingGenerator, RemoteEmbeddingGenerator},
    file_parser::file_parser::HanzoFileParser,
    source::DistributionInfo,
};

#[tokio::test]
async fn local_pdf_parsing_test() {
    HanzoFileParser::initialize_local_file_parser().await.unwrap();

    let generator = RemoteEmbeddingGenerator::new_default();
    let source_file_name = "hanzo_intro.pdf";
    let buffer = std::fs::read(format!("../../files/{}", source_file_name)).unwrap();
    let resource = HanzoFileParser::process_file_into_resource(
        buffer,
        &generator,
        source_file_name.to_string(),
        None,
        &vec![],
        generator.model_type().max_input_token_count() as u64,
        DistributionInfo::new_empty(),
    )
    .await
    .unwrap();

    resource
        .as_trait_object()
        .print_all_nodes_exhaustive(None, false, false);

    // Perform vector search
    let query_string = "What is Hanzo?".to_string();
    let query_embedding = generator.generate_embedding_default(&query_string).await.unwrap();
    let results = resource.as_trait_object().vector_search(query_embedding, 3);

    assert!(results[0].score > 0.7);
}
