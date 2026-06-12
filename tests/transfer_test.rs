use beam_rs_webrtc::core::transfer::{
    CHUNK_SIZE, ControlSignal, FileHeader, TransferType, recv_chunk, recv_control, recv_header,
    send_ack, send_chunk, send_header, send_proceed, send_resume,
};
use tokio::io::duplex;

#[tokio::test]
async fn test_header_roundtrip() {
    let (mut client, mut server) = duplex(4096);
    let header = FileHeader::new(TransferType::File, "test_file.txt".to_string(), 12345, 0);

    let send_handle = tokio::spawn(async move { send_header(&mut client, &header).await });

    let received = recv_header(&mut server).await.unwrap();
    send_handle.await.unwrap().unwrap();

    assert_eq!(received.filename, "test_file.txt");
    assert_eq!(received.file_size, 12345);
}

#[tokio::test]
async fn test_single_chunk_roundtrip() {
    let (mut client, mut server) = duplex(4096);
    let data = b"Hello, World! This is test data for a single chunk.";

    let data_clone = data.to_vec();
    let send_handle = tokio::spawn(async move { send_chunk(&mut client, &data_clone).await });

    let received = recv_chunk(&mut server).await.unwrap();
    send_handle.await.unwrap().unwrap();

    assert_eq!(received, data);
}

#[tokio::test]
async fn test_multi_chunk_roundtrip() {
    let (mut client, mut server) = duplex(65536);

    let chunks: Vec<Vec<u8>> = vec![
        b"First chunk of data".to_vec(),
        b"Second chunk of data".to_vec(),
        b"Third chunk of data".to_vec(),
    ];

    let chunks_clone = chunks.clone();
    let send_handle = tokio::spawn(async move {
        for chunk in chunks_clone.iter() {
            send_chunk(&mut client, chunk).await.unwrap();
        }
    });

    for expected in chunks.iter() {
        let received = recv_chunk(&mut server).await.unwrap();
        assert_eq!(&received, expected);
    }

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_full_transfer_simulation() {
    let (mut client, mut server) = duplex(65536);

    let filename = "document.pdf".to_string();
    let file_data = b"This is the content of the file being transferred.";
    let file_size = file_data.len() as u64;

    let filename_clone = filename.clone();
    let file_data_clone = file_data.to_vec();
    let send_handle = tokio::spawn(async move {
        let header = FileHeader::new(TransferType::File, filename_clone, file_size, 0);
        send_header(&mut client, &header).await.unwrap();
        send_chunk(&mut client, &file_data_clone).await.unwrap();
    });

    let received_header = recv_header(&mut server).await.unwrap();
    assert_eq!(received_header.filename, filename);
    assert_eq!(received_header.file_size, file_size);

    let received_data = recv_chunk(&mut server).await.unwrap();
    assert_eq!(received_data, file_data);

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_large_file_multi_chunk() {
    let file_size = CHUNK_SIZE * 2 + 1000;
    let (mut client, mut server) = duplex(file_size + 4096);

    let file_data: Vec<u8> = (0..file_size).map(|i| (i % 256) as u8).collect();

    let file_data_clone = file_data.clone();
    let send_handle = tokio::spawn(async move {
        let header = FileHeader::new(
            TransferType::File,
            "large_file.bin".to_string(),
            file_size as u64,
            0,
        );
        send_header(&mut client, &header).await.unwrap();

        for chunk in file_data_clone.chunks(CHUNK_SIZE) {
            send_chunk(&mut client, chunk).await.unwrap();
        }
    });

    let received_header = recv_header(&mut server).await.unwrap();
    assert_eq!(received_header.file_size, file_size as u64);

    let mut received_data = Vec::new();
    while received_data.len() < file_size {
        let chunk = recv_chunk(&mut server).await.unwrap();
        received_data.extend(chunk);
    }

    assert_eq!(received_data, file_data);

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_exact_chunk_size_file() {
    let (mut client, mut server) = duplex(CHUNK_SIZE + 1024);

    let file_data: Vec<u8> = (0..CHUNK_SIZE).map(|i| (i % 256) as u8).collect();
    let file_size = file_data.len() as u64;

    let file_data_clone = file_data.clone();
    let send_handle = tokio::spawn(async move {
        let header = FileHeader::new(TransferType::File, "exact_chunk.bin".to_string(), file_size, 0);
        send_header(&mut client, &header).await.unwrap();
        send_chunk(&mut client, &file_data_clone).await.unwrap();
    });

    let received_header = recv_header(&mut server).await.unwrap();
    assert_eq!(received_header.file_size, CHUNK_SIZE as u64);

    let received_data = recv_chunk(&mut server).await.unwrap();
    assert_eq!(received_data, file_data);

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_empty_file_header_roundtrip() {
    let (mut client, mut server) = duplex(4096);
    let filename = "empty.txt".to_string();

    let filename_clone = filename.clone();
    let send_handle = tokio::spawn(async move {
        let header = FileHeader::new(TransferType::File, filename_clone, 0, 0);
        send_header(&mut client, &header).await.unwrap();
    });

    let received_header = recv_header(&mut server).await.unwrap();
    assert_eq!(received_header.filename, filename);
    assert_eq!(received_header.file_size, 0);

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_folder_transfer_type() {
    let (mut client, mut server) = duplex(4096);
    let header = FileHeader::new(TransferType::Folder, "myfolder.tar".to_string(), 54321, 0);

    let send_handle = tokio::spawn(async move { send_header(&mut client, &header).await });

    let received = recv_header(&mut server).await.unwrap();
    send_handle.await.unwrap().unwrap();

    assert_eq!(received.transfer_type, TransferType::Folder);
    assert_eq!(received.filename, "myfolder.tar");
    assert_eq!(received.file_size, 54321);
}

#[tokio::test]
async fn test_special_characters_in_filename() {
    let (mut client, mut server) = duplex(4096);
    let filename = "file with spaces & special (chars) [2024].txt".to_string();

    let header = FileHeader::new(TransferType::File, filename.clone(), 100, 0);
    let send_handle = tokio::spawn(async move {
        send_header(&mut client, &header).await.unwrap();
    });

    let received = recv_header(&mut server).await.unwrap();
    assert_eq!(received.filename, filename);

    send_handle.await.unwrap();
}

#[tokio::test]
async fn test_control_signal_roundtrips() {
    let (mut client, mut server) = duplex(4096);

    let send_handle = tokio::spawn(async move {
        send_proceed(&mut client).await.unwrap();
        send_resume(&mut client, 8192).await.unwrap();
        send_ack(&mut client).await.unwrap();
    });

    assert_eq!(recv_control(&mut server).await.unwrap(), ControlSignal::Proceed);
    assert_eq!(
        recv_control(&mut server).await.unwrap(),
        ControlSignal::Resume(8192)
    );
    assert_eq!(recv_control(&mut server).await.unwrap(), ControlSignal::Ack);

    send_handle.await.unwrap();
}
