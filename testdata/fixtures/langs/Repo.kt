package com.example.fixtures

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

interface UserRepository {
    fun getAll(): Flow<List<String>>
    suspend fun insert(user: String)
}

class UserRepositoryImpl(private val dao: Any) : UserRepository {
    override fun getAll(): Flow<List<String>> = flow { emit(listOf("a", "b")) }

    override suspend fun insert(user: String) {
        println("insert $user")
    }
}

object UserModule {
    fun provide(): UserRepository = UserRepositoryImpl(Unit)
}

fun topLevelHelper(x: Int): Int = x * 2
