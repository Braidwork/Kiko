package io.github.braidwork.kiko;

import io.github.braidwork.kiko.sdk.*;

import java.io.IOException;
import java.util.List;
import java.util.Scanner;

public class Main {
    public static void main(String[] args) {
        try (Scanner scanner = new Scanner(System.in)) {
            System.out.println("Robot id:");
            String robotId = scanner.nextLine();
            System.out.println("Main flow id:");
            String mainFlowId = scanner.nextLine();
            RequestData request = RequestData.create(robotId, mainFlowId, null);
            RequestHandler requestHandler = new RequestHandler("http://127.0.0.1:12715/flow/answer");
            // Accumulates what the callback already printed, so the response's own
            // answers — the ones that were not streamed — can be told apart from it.
            StringBuilder streamed = new StringBuilder();
            Response response;
            while (true) {
                streamed.setLength(0);
                try {
                    // The callback is what asks for a stream, and it is called as
                    // each delta lands: the answer shows up while it is generated
                    // rather than after.
                    response = requestHandler.req(request, 1000, chunk -> {
                        streamed.append(chunk.getContent());
                        System.out.print(chunk.getContent());
                        System.out.flush();
                    });
                } catch (IOException | InterruptedException e) {
                    System.out.println("Request failed, err: " + e.getMessage());
                    response = null;
                }
                if (response == null) {
                    System.out.println("Response failed, please try again.");
                } else if (response.getStatus() != 200) {
                    System.out.println("Response failed: " + response.getErr());
                }  else if (response.getData() == null) {
                    System.out.println("Request failed, please try again.");
                } else {
                    ResponseData data = response.getData();
                    if (streamed.length() > 0) {
                        System.out.println();
                    }
                    // A streamed answer came through the callback above; what is
                    // left here are the answers that were never streamed.
                    List<Answer> answers = data.getAnswers();
                    if (answers == null || answers.isEmpty()) {
                        if (streamed.length() == 0) {
                            System.out.println("No answer.");
                        }
                    } else {
                        System.out.println(answers.size() == 1 ? "Answer:" : "Answers:");
                        for (Answer answer : answers) {
                            System.out.println(answer.getContent());
                        }
                    }
                    if (NextAction.TERMINATE.equals(data.getNextAction())) {
                        System.out.println();
                        System.exit(0);
                    }
                    if (request.getSessionId() == null || request.getSessionId().isEmpty())
                        request.setSessionId(data.getSessionId());
                }
                System.out.println("Input your question:");
                request.setUserInput(scanner.nextLine());
                if (request.getUserInput().isEmpty())
                    request.setUserInputResult(UserInputResult.TIMEOUT);
                else
                    request.setUserInputResult(UserInputResult.SUCCESSFUL);
            }
        }
    }
}
