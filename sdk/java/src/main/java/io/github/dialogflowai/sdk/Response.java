package io.github.braidwork.kiko.sdk;

import lombok.Data;

@Data
public class Response {
    private int status;
    private ResponseData data;
    public String err;
}
